// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.theme

import app.g7link.android.ui.Preferences
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.w3c.dom.Element

class ThemeAndLocalizationTest {

    @Test
    fun `sanitizeAppearance keeps G7, the palettes and the modes, and falls back on anything else`() {
        val themes = listOf("quiet") + Palettes.families.map { it.id }
        for (theme in themes) {
            for (mode in listOf("system", "light", "dark")) {
                val sanitized = Preferences.sanitizeAppearance(Appearance(theme = theme, mode = mode))
                assertEquals(theme, sanitized.theme)
                assertEquals(mode, sanitized.mode)
            }
        }
        assertEquals(Appearance(), Preferences.sanitizeAppearance(theme = "unknown_theme", mode = "unknown_mode"))
        assertEquals(Appearance(), Preferences.sanitizeAppearance(theme = null, mode = null))
    }

    @Test
    fun `themes from before colors-only become G7`() {
        for (old in listOf("hive", "bloom", "glass", "blurry", "graphite")) {
            assertEquals(old, "quiet", Preferences.sanitizeAppearance(theme = old, mode = "light").theme)
        }
    }

    @Test
    fun `a dark-only palette stays dark, the others follow the mode`() {
        assertTrue(isDark(Appearance(theme = "dracula", mode = "light"), systemDark = false))
        assertFalse(isDark(Appearance(theme = "catppuccin", mode = "light"), systemDark = true))
        assertTrue(isDark(Appearance(theme = "catppuccin", mode = "system"), systemDark = true))
        assertFalse(isDark(Appearance(theme = "quiet", mode = "system"), systemDark = false))
    }

    @Test
    fun `a palette maps onto every G7 role`() {
        val mocha = Palettes.families.first { it.id == "catppuccin" }.dark
        val colors = themeColors("catppuccin", dark = true)
        assertEquals(mocha.background, colors.palette.surface)
        assertEquals(mocha.foreground, colors.palette.onSurface)
        assertEquals(mocha.border, colors.palette.outlineVariant)
        assertEquals(mocha.accent, colors.palette.primary)
        assertEquals(mocha.red, colors.error)
        assertEquals(mocha.magenta, colors.tints.pink)
        assertEquals(mocha.cyan, colors.tints.teal)
        assertEquals(mocha.purple, colors.tints.lavender)
        // The raised surface sits between the ground and the ink.
        assertTrue(colors.palette.surfaceContainer.red > mocha.background.red)
        assertTrue(colors.palette.surfaceContainer.red < colors.palette.surfaceContainerHigh.red)
        // Text on the accent is the ground or the ink.
        assertTrue(colors.palette.onPrimary == mocha.background || colors.palette.onPrimary == mocha.foreground)
        assertEquals(ThemeColors.g7Dark, themeColors("quiet", dark = true))
        assertEquals(ThemeColors.g7Light, themeColors("unknown", dark = false))
    }

    @Test
    fun `every palette has a dark variant and distinct light and dark grounds`() {
        for (family in Palettes.families) {
            val light = family.light ?: continue
            assertNotEquals(family.id, light.background, family.dark.background)
        }
    }

    private fun resolveResDir(): File {
        val candidates = listOf(
            File("src/main/res"),
            File("app/src/main/res"),
            File("android/app/src/main/res"),
        )
        return candidates.firstOrNull { it.isDirectory }
            ?: error("Could not locate src/main/res from working directory ${File(".").absolutePath}")
    }

    private fun parsePluralsXml(file: File): Map<String, Map<String, String>> {
        val doc = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(file)
        doc.documentElement.normalize()

        val plurals = linkedMapOf<String, Map<String, String>>()
        val pluralNodes = doc.getElementsByTagName("plurals")
        for (i in 0 until pluralNodes.length) {
            val el = pluralNodes.item(i) as? Element ?: continue
            if (el.getAttribute("translatable") == "false") continue
            val name = el.getAttribute("name")
            val items = linkedMapOf<String, String>()
            val itemNodes = el.getElementsByTagName("item")
            for (j in 0 until itemNodes.length) {
                val item = itemNodes.item(j) as? Element ?: continue
                items[item.getAttribute("quantity")] = item.textContent
            }
            if (name.isNotEmpty()) {
                plurals[name] = items
            }
        }
        return plurals
    }

    private val placeholderRegex = Regex("""%(?:\d+\$)?(?:\.\d+)?[sdf]""")

    private fun extractPlaceholders(text: String): Set<String> =
        placeholderRegex.findAll(text.replace("%%", "")).map { it.value }.toSet()

    @Test
    fun `Simplified Chinese localization covers all translatable plurals with matching placeholders`() {
        val resDir = resolveResDir()
        val enFile = File(resDir, "values/strings.xml")
        val zhFile = File(resDir, "values-zh-rCN/strings.xml")
        assertTrue("values/strings.xml must exist", enFile.isFile)
        assertTrue("values-zh-rCN/strings.xml must exist", zhFile.isFile)

        val enPlurals = parsePluralsXml(enFile)
        val zhPlurals = parsePluralsXml(zhFile)

        assertTrue("Expected plurals in values/strings.xml", enPlurals.isNotEmpty())

        val missingPlurals = enPlurals.keys - zhPlurals.keys
        assertTrue("Missing zh-rCN plural translations: $missingPlurals", missingPlurals.isEmpty())

        for ((key, enItems) in enPlurals) {
            val zhItems = zhPlurals.getValue(key)
            assertTrue("Empty zh-rCN plural items for '$key'", zhItems.isNotEmpty())
            val expectedPlaceholders = enItems.values.flatMap { extractPlaceholders(it) }.toSet()
            for ((qty, zhText) in zhItems) {
                assertFalse("Blank zh-rCN plural '$key' quantity '$qty'", zhText.isBlank())
                assertEquals(
                    "Placeholder mismatch in plural '$key' quantity '$qty'",
                    expectedPlaceholders,
                    extractPlaceholders(zhText),
                )
            }
        }
    }
}
