// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.ui.text.AnnotatedString
import app.g7link.core.AgentLink
import app.g7link.core.AgentSpan
import app.g7link.core.AgentStyle
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The pure parts of drawing agent text: our templates, spans made safe, durations, links. */
class AgentTextTest {
    private fun span(start: Int, len: Int, link: AgentLink = AgentLink.NONE, href: String = "") = AgentSpan(
        start = start.toUInt(),
        len = len.toUInt(),
        style = AgentStyle(strong = true, em = false, strike = false, code = false, link = link, image = false),
        href = href,
    )

    @Test
    fun `templates split into their own text and positional arguments`() {
        assertEquals(listOf(AgentText.Piece.Literal("Ran "), AgentText.Piece.Arg(0)), AgentText.pieces("Ran %1\$s"))
        assertEquals(
            listOf(AgentText.Piece.Arg(0), AgentText.Piece.Literal(" ve "), AgentText.Piece.Arg(1), AgentText.Piece.Literal(" tane daha")),
            AgentText.pieces("%1\$s ve %2\$d tane daha"),
        )
        // A translation may put the arguments in another order, and a percent sign stays one.
        assertEquals(listOf(AgentText.Piece.Arg(1), AgentText.Piece.Literal(" üzerinde 100% "), AgentText.Piece.Arg(0)), AgentText.pieces("%2\$s üzerinde 100%% %1\$s"))
        assertEquals("Ran cargo test", AgentText.format("Ran %1\$s", AnnotatedString("cargo test")).text)
        assertEquals("a and  more", AgentText.format("%1\$s and %2\$d more", AnnotatedString("a")).text)
    }

    @Test
    fun `spans outside the text, overlapping or empty are made harmless`() {
        val text = "Hello world"
        val safe = AgentText.safeSpans(text.length, listOf(span(6, 50), span(0, 5), span(3, 4), span(20, 2), span(1, 0)))
        // In order and inside the text; one that overlaps the one before starts where it ends.
        assertEquals(listOf(0..4, 5..6, 7..10), safe.map { it.first })
        assertTrue(safe.all { it.first.last < text.length })
    }

    @Test
    fun `durations read like the fold`() {
        assertEquals(AgentText.Worked(0, 0, 12), AgentText.worked(12_400L))
        assertEquals(AgentText.Worked(0, 2, 14), AgentText.worked(134_000L))
        assertEquals(AgentText.Worked(1, 3, 0), AgentText.worked(3_780_000L))
        assertEquals(AgentText.Worked(0, 0, 0), AgentText.worked(-5L))
    }

    @Test
    fun `only web and mail links open, and a foreign host shows in its ASCII form`() {
        assertTrue(AgentText.openable("https://example.com/a"))
        assertTrue(AgentText.openable("HTTP://example.com"))
        assertTrue(AgentText.openable("mailto:a@example.com"))
        assertFalse(AgentText.openable("javascript:alert(1)"))
        assertFalse(AgentText.openable("file:///etc/passwd"))
        assertFalse(AgentText.openable("intent://scan/#Intent;scheme=zxing;end"))
        assertFalse(AgentText.openable("/relative/path"))

        assertEquals("https://xn--80ak6aa92e.com/login?a=1", AgentText.shownTarget("https://аррӏе.com/login?a=1"))
        assertEquals("https://user@xn--bcher-kva.example:8443/x", AgentText.shownTarget("https://user@bücher.example:8443/x"))
        assertEquals("mailto:a@xn--bcher-kva.example?subject=x", AgentText.shownTarget("mailto:a@bücher.example?subject=x"))
        assertEquals("https://example.com/ü", AgentText.shownTarget("https://example.com/ü"))
    }

    @Test
    fun `paths split into a muted folder and the name, output keeps its last lines`() {
        assertEquals("core/ble/src/" to "hold.rs", AgentText.splitPath("core/ble/src/hold.rs"))
        assertEquals("" to "hold.rs", AgentText.splitPath("hold.rs"))
        assertEquals("c\nd" to true, AgentText.lastLines("a\nb\nc\nd", 2))
        assertEquals("a\nb" to false, AgentText.lastLines("a\nb", 5))
        assertEquals("" to false, AgentText.lastLines("", 3))
    }
}
