// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.player

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

/**
 * The player's strings live in their own strings_player.xml (so they never share a file with
 * the rest of the app's), which LocalesTest doesn't read: every folder that has a strings.xml has
 * them all, with the English placeholders.
 */
class PlayerStringsTest {

    private val res: File = listOf(File("src/main/res"), File("app/src/main/res"), File("android/app/src/main/res"))
        .firstOrNull { it.isDirectory }
        ?: error("Could not locate src/main/res from ${File(".").absolutePath}")

    private fun strings(file: File): Map<String, String> {
        val doc = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(file)
        val nodes = doc.getElementsByTagName("string")
        return (0 until nodes.length).associate { i ->
            val el = nodes.item(i) as Element
            el.getAttribute("name") to el.textContent
        }
    }

    private fun placeholders(text: String): List<String> = Regex("%\\d+\\\$[sd]").findAll(text).map { it.value }.sorted().toList()

    @Test
    fun `every language folder has every player string`() {
        val english = strings(File(res, "values/strings_player.xml"))
        assertTrue(english.isNotEmpty())
        val folders = res.listFiles { f -> f.isDirectory && f.name.startsWith("values-") && File(f, "strings.xml").isFile }.orEmpty()
        assertTrue("the six translated folders", folders.size >= 6)
        for (folder in folders) {
            val file = File(folder, "strings_player.xml")
            assertTrue("${folder.name} has no strings_player.xml", file.isFile)
            val translated = strings(file)
            assertEquals("${folder.name}'s player strings", english.keys, translated.keys)
            for ((name, text) in english) {
                val other = translated.getValue(name)
                assertTrue("${folder.name}/$name is empty", other.isNotBlank())
                assertEquals("${folder.name}/$name placeholders", placeholders(text), placeholders(other))
            }
        }
    }

    @Test
    fun `no player string is also in strings xml`() {
        val shared = strings(File(res, "values/strings.xml")).keys
        val player = strings(File(res, "values/strings_player.xml")).keys
        assertEquals(emptySet<String>(), shared intersect player)
    }
}
