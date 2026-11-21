// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** The quote chip (contract §SC9.1): the shared text form, and chips kept in the draft's text. */
class AgentQuoteTest {
    private val fixtures: File = listOf(
        File("../../core/g7-agents/tests/fixtures/scm/quote"),
        File("../core/g7-agents/tests/fixtures/scm/quote"),
        File("core/g7-agents/tests/fixtures/scm/quote"),
    ).firstOrNull { it.isDirectory } ?: error("Could not locate the quote fixtures from ${File(".").absolutePath}")

    @Test
    fun `every shared fixture writes the same text`() {
        val files = fixtures.listFiles { f -> f.name.endsWith(".json") }.orEmpty().sortedBy { it.name }
        assertEquals("the six fixtures", 6, files.size)
        for (file in files) {
            @Suppress("UNCHECKED_CAST")
            val root = MiniJson(file.readText()).value() as Map<String, Any?>

            @Suppress("UNCHECKED_CAST")
            val q = root["quote"] as Map<String, Any?>
            val quote = Quote(q["path"] as String, (q["first"] as Long).toInt(), (q["last"] as Long).toInt(), q["text"] as String)
            assertEquals(file.name, root["text"], AgentQuotes.text(quote))
        }
    }

    @Test
    fun `a fence is longer than any run of backticks inside`() {
        val quote = Quote("a.md", 1, 1, "``````x")
        assertTrue(AgentQuotes.text(quote).endsWith("\n```````"))
    }

    @Test
    fun `chips live in the draft's text, in front of the words typed`() {
        val one = Quote("src/a.rs", 3, 4, "let x = 1;\nlet y = 2;")
        val two = Quote("docs/über notes.md", 10, 10, "```sh\nrun\n```")
        val draft = AgentQuotes.compose(listOf(one, two), "Why both?")
        assertEquals(AgentQuotes.text(one) + "\n\n" + AgentQuotes.text(two) + "\n\nWhy both?", draft)
        assertEquals(listOf(one, two) to "Why both?", AgentQuotes.split(draft))
        // Typing changes the words only.
        assertEquals(listOf(one, two) to "Why?", AgentQuotes.split(AgentQuotes.typed(draft, "Why?")))
        assertEquals(listOf(one) to "", AgentQuotes.split(AgentQuotes.compose(listOf(one), "")))
    }

    @Test
    fun `a draft without chips is all words, even one that looks a little like a quote`() {
        assertEquals(emptyList<Quote>() to "hello", AgentQuotes.split("hello"))
        val almost = "a.rs:1–2\n```\nnot closed"
        assertEquals(emptyList<Quote>() to almost, AgentQuotes.split(almost))
        val underneath = AgentQuotes.text(Quote("a.rs", 1, 1, "x")) + "\nstraight under"
        assertEquals(emptyList<Quote>() to underneath, AgentQuotes.split(underneath))
    }

    @Test
    fun `at most five chips, a sixth is refused, and a chip comes out alone`() {
        var draft = "words"
        repeat(AgentQuotes.MAX) { i -> draft = AgentQuotes.add(draft, Quote("f$i.rs", i + 1, i + 1, "line $i"))!! }
        assertNull(AgentQuotes.add(draft, Quote("g.rs", 1, 1, "x")))
        val after = AgentQuotes.remove(draft, 1)
        val (left, words) = AgentQuotes.split(after)
        assertEquals(listOf("f0.rs", "f2.rs", "f3.rs", "f4.rs"), left.map { it.path })
        assertEquals("words", words)
        assertEquals(after, AgentQuotes.remove(after, 9))
    }

    @Test
    fun `lines picked in a diff become a chip from the lowest number to the highest`() {
        val quote = AgentQuotes.of("src/sync/queue.rs", listOf(QuotedLine(45, "        link.wait_ack(row.id)?;"), QuotedLine(44, "        link.send(row)?;")))!!
        assertEquals("src/sync/queue.rs:44–45", AgentQuotes.label(quote))
        assertEquals("        link.wait_ack(row.id)?;\n        link.send(row)?;", quote.text)
        assertNull("nothing numbered", AgentQuotes.of("a.rs", listOf(QuotedLine(0, "@@"))))
        assertNull("no path", AgentQuotes.of("", listOf(QuotedLine(1, "x"))))
    }

    @Test
    fun `a chip quotes at most 2,000 characters, never half a character`() {
        val long = "😀".repeat(3_000)
        val quote = AgentQuotes.of("a.txt", listOf(QuotedLine(1, long)))!!
        assertEquals(AgentQuotes.MAX_CHARS, quote.text.codePointCount(0, quote.text.length))
    }

    /** Just enough JSON for the fixtures: objects, strings, integers. */
    private class MiniJson(private val s: String) {
        private var i = 0

        fun value(): Any? {
            space()
            return when (s[i]) {
                '{' -> obj()
                '"' -> str()
                else -> num()
            }
        }

        private fun obj(): Map<String, Any?> {
            val out = LinkedHashMap<String, Any?>()
            i++
            space()
            if (s[i] == '}') return out.also { i++ }
            while (true) {
                space()
                val key = str()
                space()
                check(s[i++] == ':')
                out[key] = value()
                space()
                when (s[i++]) {
                    ',' -> continue
                    '}' -> return out
                    else -> error("bad JSON at $i")
                }
            }
        }

        private fun str(): String {
            check(s[i++] == '"')
            val out = StringBuilder()
            while (true) {
                val c = s[i++]
                when (c) {
                    '"' -> return out.toString()
                    '\\' -> when (val e = s[i++]) {
                        'n' -> out.append('\n')
                        't' -> out.append('\t')
                        'r' -> out.append('\r')
                        'u' -> {
                            out.append(s.substring(i, i + 4).toInt(16).toChar())
                            i += 4
                        }
                        else -> out.append(e)
                    }
                    else -> out.append(c)
                }
            }
        }

        private fun num(): Long {
            val start = i
            while (i < s.length && (s[i].isDigit() || s[i] == '-')) i++
            return s.substring(start, i).toLong()
        }

        private fun space() {
            while (i < s.length && s[i].isWhitespace()) i++
        }
    }
}
