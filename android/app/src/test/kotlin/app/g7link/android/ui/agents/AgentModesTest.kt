// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentCatalog
import app.g7link.core.AgentFolder
import app.g7link.core.AgentFolderMode
import app.g7link.core.AgentInfo
import app.g7link.core.AgentMode
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Every mode on the phone (plans/modes-approvals-composer.md §2.1, §2.2, §4.2). */
class AgentModesTest {
    private val claudeModes = listOf(ThreadMode.AUTO, ThreadMode.ASK, ThreadMode.ACCEPT_EDITS, ThreadMode.PLAN, ThreadMode.FULL)
    private val codexModes = listOf(ThreadMode.ASK, ThreadMode.AUTO, ThreadMode.FULL, ThreadMode.PLAN)
    private val claude = AgentAbilities(control = true, modes = claudeModes)
    private val codex = AgentAbilities(control = true, modes = codexModes)

    private fun agent(id: String, modes: List<AgentMode>) =
        AgentInfo(id = id, name = id.replaceFirstChar { it.uppercaseChar() }, installed = true, signedIn = true, loginHint = "", modes = modes)

    @Test
    fun `each agent offers its own modes in its CLI's order, every one of them on the phone`() {
        assertEquals(claudeModes, AgentModes.offered(claude))
        assertEquals(codexModes, AgentModes.offered(codex))
        // A PC that doesn't take control offers none; one that names none offers asking first.
        assertEquals(emptyList<ThreadMode>(), AgentModes.offered(claude.copy(control = false)))
        assertEquals(listOf(ThreadMode.ASK), AgentModes.offered(AgentAbilities(control = true)))
        // Any offered mode is picked, Full too; one the agent doesn't offer isn't.
        assertEquals(ThreadMode.FULL, AgentSettings.pickMode(claude, ThreadSettings(), ThreadMode.FULL).mode)
        assertEquals(ThreadMode.AUTO, AgentSettings.pickMode(codex, ThreadSettings(), ThreadMode.AUTO).mode)
        assertEquals(ThreadSettings(), AgentSettings.pickMode(codex, ThreadSettings(), ThreadMode.ACCEPT_EDITS))
        assertEquals(ThreadSettings(), AgentSettings.pickMode(claude.copy(control = false), ThreadSettings(), ThreadMode.PLAN))
    }

    @Test
    fun `the FFI's modes reach the phone in the agent's order, from a PC that takes control`() {
        val info = agent("codex", listOf(AgentMode.ASK, AgentMode.AUTO, AgentMode.FULL, AgentMode.PLAN))
        assertEquals(codexModes, AgentControlFfi.abilities(info, control = true).modes)
        assertEquals(emptyList<ThreadMode>(), AgentModes.offered(AgentControlFfi.abilities(info, control = false)))
        val catalog = AgentCatalog(agents = listOf(info), folders = emptyList(), canControl = true)
        assertEquals(codexModes, AgentModes.offered(AgentsState(catalog = catalog).abilities("codex")))
    }

    @Test
    fun `old words read as Ask and Accept edits, and every mode goes out as it came in`() {
        // The FFI reads `supervised` as ASK and `auto_edit` as ACCEPT_EDITS; the phone shows them so.
        assertEquals(ThreadMode.ASK, AgentControlFfi.mode(AgentMode.ASK))
        assertEquals(ThreadMode.ACCEPT_EDITS, AgentControlFfi.mode(AgentMode.ACCEPT_EDITS))
        assertEquals(ThreadMode.FULL, AgentControlFfi.mode(AgentMode.FULL))
        for (mode in ThreadMode.entries) assertEquals(mode, AgentControlFfi.mode(AgentControlFfi.wire(mode)))
        for (mode in AgentMode.entries) assertEquals(mode, AgentControlFfi.wire(AgentControlFfi.mode(mode)))
    }

    @Test
    fun `a new thread starts in the folder's last mode for its agent, never Plan, else asking first`() {
        assertEquals(ThreadMode.ASK, AgentModes.preselect(null, claudeModes))
        assertEquals(ThreadMode.FULL, AgentModes.preselect(ThreadMode.FULL, claudeModes))
        assertEquals(ThreadMode.ACCEPT_EDITS, AgentModes.preselect(ThreadMode.ACCEPT_EDITS, claudeModes))
        assertEquals(ThreadMode.ASK, AgentModes.preselect(ThreadMode.PLAN, claudeModes))
        // A mode the agent no longer offers falls back to asking first.
        assertEquals(ThreadMode.ASK, AgentModes.preselect(ThreadMode.ACCEPT_EDITS, codexModes))
        // Per agent: a folder used with Claude in Auto still starts Codex asking first.
        val folder = FolderUse("f1", "g7linux", modes = mapOf("claude" to ThreadMode.AUTO))
        assertEquals(ThreadMode.AUTO, AgentModes.preselect(folder, "claude", claudeModes))
        assertEquals(ThreadMode.ASK, AgentModes.preselect(folder, "codex", codexModes))
        // A new folder starts asking first.
        assertEquals(ThreadMode.ASK, AgentModes.preselect(FolderUse("f2", "new"), "claude", claudeModes))
    }

    @Test
    fun `a folder's remembered modes come from the catalog, one per agent, never Plan`() {
        val folder = AgentFolder(
            id = "f1",
            label = "g7linux",
            modes = listOf(
                AgentFolderMode(agent = "claude", mode = AgentMode.FULL),
                AgentFolderMode(agent = "codex", mode = AgentMode.PLAN),
                AgentFolderMode(agent = "claude", mode = AgentMode.ASK),
            ),
        )
        val catalog = AgentCatalog(agents = emptyList(), folders = listOf(folder, AgentFolder(id = "f2", label = "site")), canControl = true)
        val uses = NewThreadFfi.folders(catalog)
        assertEquals(mapOf("claude" to ThreadMode.FULL), uses.first { it.id == "f1" }.modes)
        assertTrue(uses.first { it.id == "f2" }.modes.isEmpty())
        // A thread names its folder by label; two folders of one label leave it unknown (asking first).
        assertEquals("f1", AgentModes.folderOf(uses, "g7linux")?.id)
        assertNull(AgentModes.folderOf(uses + FolderUse("f3", "g7linux"), "g7linux"))
        assertNull(AgentModes.folderOf(uses, "elsewhere"))
    }

    @Test
    fun `the new thread's mode is the one chosen for its agent, re-read when the agent or the folder changes`() {
        val folders = listOf(
            FolderUse("a", "alpha", modes = mapOf("claude" to ThreadMode.FULL, "codex" to ThreadMode.AUTO)),
            FolderUse("b", "beta"),
        )
        var state = NewThreadFlowModel.choose(NewThreadFlowModel.open(null, null, folders.map { it.id }), "a")
        assertEquals(ThreadMode.FULL, NewThreadFlowModel.mode(state, "claude", claudeModes, folders[0]))
        assertEquals(ThreadMode.AUTO, NewThreadFlowModel.mode(state, "codex", codexModes, folders[0]))
        state = NewThreadFlowModel.pickMode(state, "claude", ThreadMode.PLAN)
        assertEquals(ThreadMode.PLAN, NewThreadFlowModel.mode(state, "claude", claudeModes, folders[0]))
        // Another agent: its own preselection; back to Claude, the choice is there again.
        assertEquals(ThreadMode.AUTO, NewThreadFlowModel.mode(state, "codex", codexModes, folders[0]))
        assertEquals(ThreadMode.PLAN, NewThreadFlowModel.mode(state, "claude", claudeModes, folders[0]))
        // An agent that stops offering the chosen mode falls back to its preselection.
        assertEquals(ThreadMode.FULL, NewThreadFlowModel.mode(state, "claude", claudeModes - ThreadMode.PLAN, folders[0]))
        // Another folder: its own mode (none remembered: asking first).
        state = NewThreadFlowModel.choose(NewThreadFlowModel.back(state)!!, "b")
        assertNull(state.mode)
        assertEquals(ThreadMode.ASK, NewThreadFlowModel.mode(state, "claude", claudeModes, folders[1]))
        // No agent to run: asking first.
        assertEquals(ThreadMode.ASK, NewThreadFlowModel.mode(state, null, emptyList(), folders[1]))
    }

    @Test
    fun `implementing a plan offers every mode but Plan, preselected as a new thread there would start`() {
        assertEquals(listOf(ThreadMode.AUTO, ThreadMode.ASK, ThreadMode.ACCEPT_EDITS, ThreadMode.FULL), AgentModes.implementModes(claude))
        assertEquals(listOf(ThreadMode.ASK, ThreadMode.AUTO, ThreadMode.FULL), AgentModes.implementModes(codex))
        assertEquals(listOf(ThreadMode.ASK), AgentModes.implementModes(AgentAbilities(control = true, modes = listOf(ThreadMode.PLAN))))
        val folder = FolderUse("f1", "g7linux", modes = mapOf("codex" to ThreadMode.FULL))
        assertEquals(ThreadMode.FULL, AgentModes.preselect(folder, "codex", AgentModes.implementModes(codex)))
        assertEquals(ThreadMode.ASK, AgentModes.preselect(folder, "claude", AgentModes.implementModes(claude)))
        // The mode chosen is the one sent.
        assertEquals(AgentMode.FULL, AgentControlFfi.wire(ThreadMode.FULL))
    }

    @Test
    fun `configure sends any offered mode that changed, Full and Accept edits too`() {
        val now = ThreadSettings(model = "opus", effort = "high")
        assertEquals(ConfigureChange(model = null, effort = null, mode = ThreadMode.FULL, customModel = false), AgentSettings.change(now, now.copy(mode = ThreadMode.FULL)))
        assertEquals(ConfigureChange(model = null, effort = null, mode = ThreadMode.ACCEPT_EDITS, customModel = false), AgentSettings.change(now, now.copy(mode = ThreadMode.ACCEPT_EDITS)))
        assertNull(AgentSettings.change(now.copy(mode = ThreadMode.AUTO), now.copy(mode = ThreadMode.AUTO)))
    }

    @Test
    fun `the reply box's mode chip shows every mode, Full in amber, and opens the list only between turns`() {
        for (mode in ThreadMode.entries) {
            val chip = AgentModes.chip(mode, SettingsAccess.OPEN)
            assertEquals(mode, chip.mode)
            assertEquals(mode == ThreadMode.FULL, chip.warn)
        }
        // Between turns a tap opens the list; while it works it says why not; while a question waits nothing happens.
        val idle = AgentDock.composer(ComposerInput(running = false))
        assertEquals(SettingsAccess.OPEN, AgentModes.chip(ThreadMode.ASK, idle.settings).tap)
        val working = AgentDock.composer(ComposerInput(running = true, followUp = FollowUp.STEER))
        assertEquals(SettingsAccess.LATER, AgentModes.chip(ThreadMode.AUTO, working.settings).tap)
        val asked = AgentDock.composer(ComposerInput(running = true, waiting = true, question = QuestionField.OTHER))
        assertEquals(SettingsAccess.INERT, AgentModes.chip(ThreadMode.FULL, asked.settings).tap)
        assertFalse(AgentModes.warns(ThreadMode.AUTO))
    }

    @Test
    fun `a step the agent's reviewer approved names the agent`() {
        assertEquals("Codex", reviewerOf(RowsContext(emptyMap(), "g7linux", agentName = "Codex")))
        val thread = AgentThread(
            id = "t1",
            agent = "codex",
            folder = "g7linux",
            title = "Fix the test",
            state = AgentThreadState.DONE,
            mode = AgentMode.AUTO,
            fromPhone = true,
            updatedMs = 1L,
            asks = 0u,
            summary = null,
            error = null,
        )
        // No name from the catalog yet: the agent's id, as a name.
        assertEquals("Codex", reviewerOf(RowsContext(emptyMap(), "g7linux", thread = thread)))
    }
}
