// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.android.core.CoreState
import app.g7link.android.core.affectsCoreState
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentFolder
import app.g7link.core.AgentMode
import app.g7link.core.AgentRunBrief
import app.g7link.core.AgentRunPhase
import app.g7link.core.AgentScript
import app.g7link.core.AgentScriptRun
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** What a PC's script events do to its agents' state on the phone, and the arms that keep them out of Home (phase 6 §X4, §X5.2). */
class AgentScriptsStateTest {
    private fun thread(runs: List<AgentRunBrief> = emptyList()) = AgentThread(
        id = "t1",
        agent = "claude",
        folder = "duka-pos",
        title = "Stock sync drops rows on reconnect",
        state = AgentThreadState.DONE,
        mode = AgentMode.SUPERVISED,
        fromPhone = false,
        updatedMs = 1_000L,
        asks = 0u,
        summary = null,
        error = null,
        runs = runs,
    )

    private fun run(phase: AgentRunPhase = AgentRunPhase.RUNNING, lines: List<String> = emptyList(), from: ULong = 0uL, mine: Boolean = true) = AgentScriptRun(
        id = "0123456789abcdef",
        thread = "t1",
        script = "s0123abcd",
        name = "dev",
        phase = phase,
        exit = null,
        startedMs = 1_000uL,
        endedMs = null,
        by = null,
        mine = mine,
        lines = lines,
        from = from,
        dropped = 0u,
        reason = null,
    )

    private fun catalog(phoneScripts: Boolean) = AgentCatalog(
        agents = emptyList(),
        folders = listOf(
            AgentFolder(
                id = "f1",
                label = "duka-pos",
                phoneScripts = phoneScripts,
                scriptCount = 1u,
                scripts = listOf(AgentScript("s0123abcd", "dev", "npm run dev")),
            ),
        ),
        modes = listOf(AgentMode.SUPERVISED),
    )

    @Test
    fun `a run's lines arrive in order and are kept with the PC's state`() {
        var state = AgentsModel.reduce(AgentsState(), Event.AgentScriptRun("pc1", run(lines = listOf("a", "b")))).state
        state = AgentsModel.reduce(state, Event.AgentScriptRun("pc1", run(lines = listOf("c"), from = 2uL))).state
        state = AgentsModel.reduce(state, Event.AgentScriptRun("pc1", run(phase = AgentRunPhase.DONE, from = 3uL))).state
        val kept = state.scriptRuns.getValue("0123456789abcdef")
        assertEquals(listOf("a", "b", "c"), kept.lines)
        assertEquals(RunPhase.DONE, kept.phase)
        assertTrue(kept.mine)
        assertEquals("pc1", AgentsModel.pcOf(Event.AgentScriptRun("pc1", run())))
    }

    @Test
    fun `a run's event is the agents' state's, not Home's`() {
        val event = Event.AgentScriptRun("pc1", run())
        assertFalse(event.affectsCoreState())
        val home = CoreState()
        assertSame(home, home.reduce(event))
    }

    @Test
    fun `a thread's runs come from its list and go with it`() {
        val brief = AgentRunBrief(id = "0123456789abcdef", name = "dev", phase = AgentRunPhase.RUNNING, exit = null)
        var state = AgentsModel.reduce(AgentsState(), Event.AgentThreadChanged("pc1", thread(runs = listOf(brief)))).state
        val listed = state.runsOf("t1").single()
        assertTrue(listed.brief)
        assertEquals(RunPhase.RUNNING, listed.phase)
        state = AgentsModel.reduce(state, Event.AgentScriptRun("pc1", run(lines = listOf("ready")))).state
        assertEquals(listOf("ready"), state.runsOf("t1").single().lines)
        state = AgentsModel.reduce(state, Event.AgentThreadRemoved("pc1", "t1")).state
        assertTrue(state.scriptRuns.isEmpty())
    }

    @Test
    fun `a folder whose switch is off keeps no command on the phone`() {
        var state = AgentsModel.reduce(AgentsState(), Event.AgentCatalog("pc1", catalog(phoneScripts = false))).state
        state = AgentsModel.reduce(state, Event.AgentThreadChanged("pc1", thread())).state
        val off = state.scriptsFolderOf("t1")!!
        assertFalse(off.phoneScripts)
        assertEquals(1, off.count)
        assertTrue("even if a command came with it", off.scripts.isEmpty())
        state = AgentsModel.reduce(state, Event.AgentCatalog("pc1", catalog(phoneScripts = true))).state
        assertEquals(listOf(Script("s0123abcd", "dev", "npm run dev")), state.scriptsFolderOf("t1")?.scripts)
        assertNull("a thread this phone doesn't know", state.scriptsFolderOf("t2"))
    }
}
