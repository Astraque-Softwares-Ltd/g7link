// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.clipboard

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ClipPreviewTest {

    private val now = 1_800_000_000_000L

    @Test
    fun `no time from Android says nothing`() {
        assertNull(ClipPreview.copied(null, now))
        assertNull(ClipPreview.copied(0L, now))
        assertNull(ClipPreview.copied(-5L, now))
    }

    @Test
    fun `under a minute ago is just now`() {
        assertEquals(ClipPreview.Copied.JustNow, ClipPreview.copied(now, now))
        assertEquals(ClipPreview.Copied.JustNow, ClipPreview.copied(now - 59_999L, now))
    }

    @Test
    fun `a minute or more ago keeps its time`() {
        assertEquals(ClipPreview.Copied.At(now - 60_000L), ClipPreview.copied(now - 60_000L, now))
        assertEquals(ClipPreview.Copied.At(now - 3_600_000L), ClipPreview.copied(now - 3_600_000L, now))
    }

    @Test
    fun `a time a little ahead is just now, one well ahead says nothing`() {
        assertEquals(ClipPreview.Copied.JustNow, ClipPreview.copied(now + 30_000L, now))
        assertNull(ClipPreview.copied(now + 61_000L, now))
    }

    @Test
    fun `short text shows as it is`() {
        assertEquals("hello", ClipPreview.lines("hello"))
        assertEquals("one\ntwo", ClipPreview.lines("one\r\ntwo"))
    }

    @Test
    fun `blank lines around the text and trailing spaces are dropped`() {
        assertEquals("  indented\nnext", ClipPreview.lines("\n \n  indented   \nnext\n\n  \n"))
    }

    @Test
    fun `only the first lines show, with an ellipsis when there are more`() {
        val text = (1..10).joinToString("\n") { "line $it" }
        assertEquals("line 1\nline 2\nline 3\nline 4\nline 5\nline 6…", ClipPreview.lines(text))
        assertEquals("a\nb…", ClipPreview.lines("a\nb\nc", maxLines = 2))
    }

    @Test
    fun `exactly the line limit has no ellipsis, nor does trailing blank space after it`() {
        assertEquals("a\nb", ClipPreview.lines("a\nb", maxLines = 2))
        assertEquals("a\nb", ClipPreview.lines("a\nb\n\n   \n", maxLines = 2))
    }

    @Test
    fun `a blank line inside the limit before more text still says there is more`() {
        assertEquals("a…", ClipPreview.lines("a\n\nb", maxLines = 2))
    }

    @Test
    fun `long text is cut to the character limit`() {
        assertEquals("abcde…", ClipPreview.lines("abcdefghij", maxChars = 5))
        assertEquals("ab…", ClipPreview.lines("ab   cdef", maxChars = 5))
    }

    @Test
    fun `a cut never splits an emoji`() {
        val text = "ab😀cd"
        assertEquals("ab…", ClipPreview.lines(text, maxChars = 3))
        assertEquals("ab…", ClipPreview.oneLine(text, maxChars = 3))
    }

    @Test
    fun `blank text shows nothing`() {
        assertEquals("", ClipPreview.lines(" \n\t\n"))
        assertEquals("", ClipPreview.oneLine(" \n\t\n"))
    }

    @Test
    fun `one line joins lines and runs of spaces`() {
        assertEquals("first line second line", ClipPreview.oneLine("  first line\n\n  second\tline  "))
    }

    @Test
    fun `one line is cut with an ellipsis only when there is more`() {
        assertEquals("abcde", ClipPreview.oneLine("abcde", maxChars = 5))
        assertEquals("abcde…", ClipPreview.oneLine("abcdef", maxChars = 5))
        assertEquals("ab…", ClipPreview.oneLine("ab   cd", maxChars = 3))
    }

    @Test
    fun `characters count an emoji as one`() {
        assertEquals(3, ClipPreview.characters("a😀b"))
        assertEquals(0, ClipPreview.characters(""))
    }

    @Test
    fun `image formats get short names`() {
        assertEquals("PNG", ClipPreview.imageFormat("image/png"))
        assertEquals("JPEG", ClipPreview.imageFormat("image/jpeg"))
        assertEquals("JPEG", ClipPreview.imageFormat("IMAGE/JPG"))
        assertEquals("WEBP", ClipPreview.imageFormat("image/webp"))
        assertEquals("SVG", ClipPreview.imageFormat("image/svg+xml"))
        assertEquals("BMP", ClipPreview.imageFormat("image/x-bmp"))
        assertNull(ClipPreview.imageFormat("text/plain"))
        assertNull(ClipPreview.imageFormat(null))
        assertNull(ClipPreview.imageFormat("image/"))
    }

    @Test
    fun `image details join what is known`() {
        assertEquals("PNG · 1080 × 2400 · 1.2 MB", ClipPreview.imageDetails("PNG", 1080, 2400, "1.2 MB"))
        assertEquals("1080 × 2400", ClipPreview.imageDetails(null, 1080, 2400, null))
        assertEquals("JPEG · 40 kB", ClipPreview.imageDetails("JPEG", 0, 0, "40 kB"))
        assertEquals("", ClipPreview.imageDetails(null, 0, 10, null))
    }
}
