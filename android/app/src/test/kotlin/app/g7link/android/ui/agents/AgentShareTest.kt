// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentShareTest {
    @Test
    fun `short markdown goes as text and long markdown as a file`() {
        assertFalse(AgentShare.asFile("# Fix the login test\n"))
        assertFalse(AgentShare.asFile("x".repeat(AgentShare.MAX_TEXT_BYTES)))
        assertTrue(AgentShare.asFile("x".repeat(AgentShare.MAX_TEXT_BYTES + 1)))
        // Counted in UTF-8 bytes, not characters.
        assertTrue(AgentShare.asFile("é".repeat(AgentShare.MAX_TEXT_BYTES / 2 + 1)))
    }

    @Test
    fun `the file is named after the pc's suggestion and stays a plain md file`() {
        assertEquals("claude-fix-it-2026-11-19.md", AgentShare.fileName("claude-fix-it-2026-11-19.md"))
        assertEquals("etcpasswd.md", AgentShare.fileName("../etc/passwd"))
        assertEquals("transcript.md", AgentShare.fileName("..."))
        assertEquals("transcript.md", AgentShare.fileName(""))
        assertEquals("notes.md", AgentShare.fileName("notes"))
        assertTrue(AgentShare.fileName("a".repeat(500)).length <= 131)
    }
}
