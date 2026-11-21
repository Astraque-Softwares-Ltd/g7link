// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import app.g7link.android.ui.theme.AgentPalette
import app.g7link.android.ui.theme.G7Colors
import app.g7link.android.ui.theme.G7TintTokens
import app.g7link.android.ui.theme.G7Tints
import app.g7link.android.ui.theme.Palettes
import app.g7link.android.ui.theme.ThemeColors
import app.g7link.android.ui.theme.toThemeColors
import app.g7link.core.AgentCodeLine
import app.g7link.core.AgentRowStatus
import app.g7link.core.AgentTok
import app.g7link.core.AgentTokClass
import app.g7link.core.AgentToolClass
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The colour system of plans/agent-colour.md on the phone: every class mapped, every text legible. */
class AgentColoursTest {
    private val dark = AgentPalette.dark
    private val light = AgentPalette.light

    /** §2.4's table, dark, written out from the plan rather than read from the palette. */
    private fun planDark(cls: AgentTokClass): Long = when (cls) {
        AgentTokClass.PLAIN -> 0xFFF2F2F3
        AgentTokClass.VARIABLE -> 0xFFD6C7E7
        AgentTokClass.OPERATOR -> 0xFFBDBDC4
        AgentTokClass.PUNCTUATION -> 0xFFA1A1A8
        AgentTokClass.COMMENT -> 0xFFA1A1A8
        AgentTokClass.KEYWORD, AgentTokClass.TAG -> 0xFFED93B1
        AgentTokClass.STRING -> 0xFF5DCAA5
        AgentTokClass.NUMBER, AgentTokClass.CONSTANT -> 0xFFF0997B
        AgentTokClass.FUNCTION -> 0xFF7CB7EE
        // Decision 2: types and attributes are amber inside code.
        AgentTokClass.TYPE, AgentTokClass.ATTRIBUTE -> 0xFFEF9F27
        AgentTokClass.INSERTED -> 0xFF97C459
        AgentTokClass.DELETED -> 0xFFE5736F
        AgentTokClass.DIR -> 0xFFA1A1A8
        AgentTokClass.FILE -> 0xFFF2F2F3
        AgentTokClass.POS -> 0xFFA1A1A8
        AgentTokClass.ERROR -> 0xFFF0A39C
        AgentTokClass.WARN -> 0xFFEF9F27
        AgentTokClass.OK -> 0xFF97C459
    }

    /** §2.4's table, light. */
    private fun planLight(cls: AgentTokClass): Long = when (cls) {
        AgentTokClass.PLAIN -> 0xFF1C1C1F
        AgentTokClass.VARIABLE -> 0xFF4F3A73
        AgentTokClass.OPERATOR -> 0xFF45454C
        AgentTokClass.PUNCTUATION -> 0xFF5F5F67
        AgentTokClass.COMMENT -> 0xFF5F5F67
        AgentTokClass.KEYWORD, AgentTokClass.TAG -> 0xFFAB436E
        AgentTokClass.STRING -> 0xFF1C755C
        AgentTokClass.NUMBER, AgentTokClass.CONSTANT -> 0xFFA74D2C
        AgentTokClass.FUNCTION -> 0xFF2E6AA9
        AgentTokClass.TYPE, AgentTokClass.ATTRIBUTE -> 0xFF8F5B00
        AgentTokClass.INSERTED -> 0xFF4A7316
        AgentTokClass.DELETED -> 0xFF8E1D0C
        AgentTokClass.DIR -> 0xFF5F5F67
        AgentTokClass.FILE -> 0xFF1C1C1F
        AgentTokClass.POS -> 0xFF5F5F67
        AgentTokClass.ERROR -> 0xFF8E1D0C
        AgentTokClass.WARN -> 0xFF8F5B00
        AgentTokClass.OK -> 0xFF4A7316
    }

    @Test
    fun `every token class has the plan's colour in both themes`() {
        for (cls in AgentTokClass.entries) {
            assertEquals("dark $cls", Color(planDark(cls)), AgentColours.tok(cls, dark))
            assertEquals("light $cls", Color(planLight(cls)), AgentColours.tok(cls, light))
        }
    }

    @Test
    fun `comments are italic, keywords and tags semibold, everything else plain in face`() {
        assertEquals(AgentTokClass.entries.size, AgentTokClass.entries.map { AgentColours.word(it) }.toSet().size)
        for (cls in AgentTokClass.entries) {
            val style = AgentColours.tokStyle(cls, dark)
            assertEquals(AgentColours.tok(cls, dark), style.color)
            assertEquals(cls.toString(), if (cls == AgentTokClass.COMMENT) FontStyle.Italic else null, style.fontStyle)
            val bold = cls == AgentTokClass.KEYWORD || cls == AgentTokClass.TAG
            assertEquals(cls.toString(), if (bold) FontWeight.SemiBold else null, style.fontWeight)
        }
    }

    @Test
    fun `every code colour reads at 4_5 to 1 on the code ground and the chip, in both themes`() {
        for (p in listOf(dark, light)) {
            for (cls in AgentTokClass.entries) {
                for (ground in listOf(p.groundCode, p.groundChip)) {
                    val ratio = AgentColours.contrast(AgentColours.tok(cls, p), ground)
                    assertTrue("$cls on $ground: $ratio", ratio >= AgentColours.TEXT_RATIO)
                }
            }
        }
    }

    @Test
    fun `diff text reads on every wash, numbers and signs on the rows, and the added row is the lighter one`() {
        for (p in listOf(dark, light)) {
            for (wash in listOf(p.diffAddRow, p.diffDelRow, p.diffAddWord, p.diffDelWord)) {
                for (ground in listOf(p.groundCode, p.groundChip)) {
                    val shown = AgentColours.over(wash, ground)
                    assertTrue("text on $wash", AgentColours.contrast(p.diffText, shown) >= AgentColours.TEXT_RATIO)
                }
            }
            // Line numbers and signs sit on the row washes, never on a word mark.
            for (wash in listOf(p.diffAddRow, p.diffDelRow)) {
                val shown = AgentColours.over(wash, p.groundCode)
                for (color in listOf(p.diffLineNo, p.diffAddSign, p.diffDelSign)) {
                    assertTrue("$color on $wash", AgentColours.contrast(color, shown) >= AgentColours.TEXT_RATIO)
                }
            }
            val added = AgentColours.over(p.diffAddRow, p.groundCode).luminance()
            val removed = AgentColours.over(p.diffDelRow, p.groundCode).luminance()
            assertTrue("added $added, removed $removed", added > removed)
        }
    }

    @Test
    fun `each kind of step has its glyph tint, and the ok check shows on commands only`() {
        assertEquals(Color(0xFF5DCAA5), AgentColours.toolGlyph(AgentToolClass.COMMAND, dark))
        assertEquals(Color(0xFFF0997B), AgentColours.toolGlyph(AgentToolClass.EDIT, dark))
        assertEquals(Color(0xFF7CB7EE), AgentColours.toolGlyph(AgentToolClass.READ, dark))
        assertEquals(Color(0xFFED93B1), AgentColours.toolGlyph(AgentToolClass.SEARCH, dark))
        assertEquals(Color(0xFF7CB7EE), AgentColours.toolGlyph(AgentToolClass.WEB, dark))
        assertEquals(Color(0xFFB292D0), AgentColours.toolGlyph(AgentToolClass.AGENT, dark))
        for (cls in listOf(AgentToolClass.MCP, AgentToolClass.TASKS, AgentToolClass.PLAN, AgentToolClass.OTHER, null)) {
            assertEquals("$cls", Color(0xFFA1A1A8), AgentColours.toolGlyph(cls, dark))
        }
        for (cls in AgentToolClass.entries) assertEquals("$cls", cls == AgentToolClass.COMMAND, AgentColours.showsOk(cls))
        assertEquals(dark.toolStatusOk, AgentColours.toolStatus(AgentRowStatus.OK, dark))
        assertEquals(dark.toolStatusFailed, AgentColours.toolStatus(AgentRowStatus.FAILED, dark))
        assertEquals(dark.toolStatusRunning, AgentColours.toolStatus(AgentRowStatus.RUNNING, dark))
        assertEquals(dark.toolStatusAsking, AgentColours.toolStatus(AgentRowStatus.ASKING, dark))
        assertEquals(dark.toolStatusDenied, AgentColours.toolStatus(AgentRowStatus.DENIED, dark))
        assertEquals(dark.toolStatusStopped, AgentColours.toolStatus(AgentRowStatus.STOPPED, dark))
    }

    @Test
    fun `done and plan ready are green, as ok and added are`() {
        for (p in listOf(dark, light)) {
            assertEquals(p.syntaxInserted, AgentColours.status(ThreadStatus.Done, p))
            assertEquals(p.statusDone, AgentColours.status(ThreadStatus.PlanReady, p))
            assertEquals(p.toolStatusOk, p.statusDone)
            assertEquals(p.statusNeedsYou, AgentColours.status(ThreadStatus.NeedsYou, p))
            assertEquals(p.statusWorking, AgentColours.status(ThreadStatus.Working, p))
            assertEquals(p.statusFailed, AgentColours.status(ThreadStatus.Failed(null), p))
            assertEquals(p.statusPaused, AgentColours.status(ThreadStatus.Paused(null), p))
            assertEquals(p.statusPaused, AgentColours.status(ThreadStatus.Stopped, p))
        }
    }

    private fun inks(colors: ThemeColors) = ThemeInks(
        surface = colors.palette.surface,
        code = colors.palette.surfaceContainer,
        chip = colors.palette.surfaceContainerHigh,
        ink = colors.palette.onSurface,
        muted = colors.palette.onSurfaceVariant,
        outline = colors.palette.outline,
        link = colors.palette.primary,
        tints = colors.tints,
    )

    @Test
    fun `G7's theme gets the generated palette, and its tints are the tokens'`() {
        assertSame(AgentPalette.dark, AgentColours.palette(inks(ThemeColors.g7Dark)))
        assertSame(AgentPalette.light, AgentColours.palette(inks(ThemeColors.g7Light)))
        assertEquals(G7TintTokens.dark, G7Tints.dark)
        assertEquals(G7TintTokens.light, G7Tints.light)
        assertEquals(G7Colors.darkPalette.surfaceContainer, AgentPalette.dark.groundCode)
        assertEquals(G7Colors.lightPalette.surfaceContainer, AgentPalette.light.groundCode)
    }

    @Test
    fun `on every bundled palette each role is made legible on the ground it meets`() {
        val variants = Palettes.families.flatMap { family -> listOfNotNull(family.dark, family.light).map { family.id to it } }
        assertTrue(variants.isNotEmpty())
        for ((id, colors) in variants) {
            val theme = colors.toThemeColors()
            val p = AgentColours.palette(inks(theme))
            for (cls in AgentTokClass.entries) {
                for (ground in listOf(p.groundCode, p.groundChip)) {
                    val ratio = AgentColours.contrast(AgentColours.tok(cls, p), ground)
                    assertTrue("$id $cls: $ratio", ratio >= AgentColours.TEXT_RATIO - 0.01f)
                }
            }
            val lineText = listOf(
                p.statusDone, p.statusFailed, p.statusFailedText, p.statusNeedsYou, p.statusWorking,
                p.toolStatusOk, p.toolStatusFailed, p.toolStatusRunning, p.toolStatusAsking,
                p.proseLink, p.proseAlertNote, p.proseAlertWarning, p.proseAlertCaution, p.proseDoneBox,
            )
            for (color in lineText) {
                for (ground in listOf(p.groundSurface, p.groundCode)) {
                    val ratio = AgentColours.contrast(color, ground)
                    assertTrue("$id $color: $ratio", ratio >= AgentColours.TEXT_RATIO - 0.01f)
                }
            }
            assertEquals(theme.palette.surfaceContainer, p.groundCode)
        }
    }

    @Test
    fun `legible keeps a colour that passes and moves one that doesn't towards the ink`() {
        val ground = Color(0xFF222226)
        assertEquals(Color(0xFF5DCAA5), AgentColours.legible(Color(0xFF5DCAA5), ground))
        val dim = Color(0xFF3A3A40)
        val moved = AgentColours.legible(dim, ground)
        assertTrue(AgentColours.contrast(moved, ground) >= AgentColours.TEXT_RATIO)
        assertTrue("lighter on a dark ground", moved.luminance() > dim.luminance())
        val onLight = AgentColours.legible(Color(0xFFE0E0E0), Color(0xFFF1F1F3))
        assertTrue(onLight.luminance() < Color(0xFFE0E0E0).luminance())
    }

    // ---- Drawing the runs ----

    private val styleOf: (AgentTokClass) -> SpanStyle = { cls -> AgentColours.tokStyle(cls, dark) }

    @Test
    fun `a tool line's runs cover its target and colour each part`() {
        val target = "src/sync/queue.rs:12"
        val toks = listOf(AgentTok(9u, AgentTokClass.DIR), AgentTok(8u, AgentTokClass.FILE), AgentTok(3u, AgentTokClass.POS))
        val runs = AgentText.runs(target, toks, styleOf)
        assertEquals(target, runs.text)
        assertEquals(listOf(0 until 9, 9 until 17, 17 until 20), runs.spanStyles.map { it.start until it.end })
        assertEquals(listOf(dark.syntaxDir, dark.syntaxFile, dark.syntaxPos), runs.spanStyles.map { it.item.color })
        // Runs past the end are cut; text past the runs stays plain.
        val short = AgentText.runs("cargo test", listOf(AgentTok(5u, AgentTokClass.FUNCTION), AgentTok(99u, AgentTokClass.PLAIN)), styleOf)
        assertEquals("cargo test", short.text)
        assertEquals(1, short.spanStyles.size)
    }

    @Test
    fun `output keeps its last lines, marked when the lines spell it and plain when they don't`() {
        val text = "Compiling g7\nwarning: unused\nerror[E0425]: cannot find\n"
        val lines = listOf(
            AgentCodeLine("Compiling g7", emptyList()),
            AgentCodeLine("warning: unused", listOf(AgentTok(15u, AgentTokClass.WARN))),
            AgentCodeLine("error[E0425]: cannot find", listOf(AgentTok(25u, AgentTokClass.ERROR))),
        )
        val (tail, cut) = AgentText.markedTail(text, lines, 2, styleOf)
        assertEquals("warning: unused\nerror[E0425]: cannot find", tail.text)
        assertTrue(cut)
        assertEquals(listOf(dark.syntaxWarn, dark.syntaxError), tail.spanStyles.map { it.item.color })
        val other = AgentText.markedTail("something else", lines, 2, styleOf)
        assertEquals("something else", other.first.text)
        assertTrue("plain when they don't match", other.first.spanStyles.isEmpty())
        val plain = AgentText.markedTail(text, emptyList(), 5, styleOf)
        assertEquals(text.trimEnd('\n'), plain.first.text)
        assertNull(plain.first.spanStyles.firstOrNull())
    }
}
