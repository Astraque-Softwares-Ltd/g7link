// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.android.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AgentMarksTest {
    @Test
    fun claudeAndCodexHaveTheirOwnMarks() {
        assertEquals(AgentMark.CLAUDE, AgentMark.of("claude"))
        assertEquals(AgentMark.OPENAI, AgentMark.of("codex"))
    }

    @Test
    fun anyOtherAgentKeepsItsNameAlone() {
        assertNull(AgentMark.of("gemini"))
        assertNull(AgentMark.of(""))
    }

    @Test
    fun theBlossomIsWhiteOnDarkAndBlackOnLight() {
        assertEquals(R.drawable.ic_agent_openai_white, AgentMark.OPENAI.drawable(dark = true))
        assertEquals(R.drawable.ic_agent_openai_black, AgentMark.OPENAI.drawable(dark = false))
    }

    @Test
    fun theSparkIsClayOnBothThemes() {
        assertEquals(R.drawable.ic_agent_claude, AgentMark.CLAUDE.drawable(dark = true))
        assertEquals(R.drawable.ic_agent_claude, AgentMark.CLAUDE.drawable(dark = false))
    }
}
