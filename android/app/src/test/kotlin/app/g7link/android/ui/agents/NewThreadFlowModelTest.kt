// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The new-thread flow, direction C (plans/new-thread-c-contract.md §5). */
class NewThreadFlowModelTest {
    private val folders = listOf("orbit-api", "dashboard")

    @Test
    fun `back goes from Browse and What to Where, and Where closes`() {
        val where = NewThreadFlowModel.open(null, null, folders)
        assertNull(NewThreadFlowModel.back(where))
        val browsing = NewThreadFlowModel.browse(where)
        assertEquals(FlowStep.BROWSE, browsing.step)
        assertEquals(FlowStep.WHERE, NewThreadFlowModel.back(browsing)?.step)
        val what = NewThreadFlowModel.choose(where, "orbit-api")
        assertEquals(FlowStep.WHAT, what.step)
        val back = NewThreadFlowModel.back(what)
        assertEquals(FlowStep.WHERE, back?.step)
        // Where still knows the folder chosen: its card is the marked one.
        assertEquals("orbit-api", back?.folderId)
    }

    @Test
    fun `a plain new thread always opens on Where, even with a folder to remember`() {
        val state = NewThreadFlowModel.open(null, "orbit-api", folders)
        assertEquals(FlowStep.WHERE, state.step)
        assertNull(state.folderId)
        assertFalse(state.shared)
        assertEquals("", state.prompt)
    }

    @Test
    fun `a share lands on What in the folder last started in while it's still there`() {
        val state = NewThreadFlowModel.open(SharedDraft("Look at this crash"), "dashboard", folders)
        assertEquals(FlowStep.WHAT, state.step)
        assertEquals("dashboard", state.folderId)
        assertEquals("Look at this crash", state.prompt)
        assertFalse(state.shared)
    }

    @Test
    fun `a share whose folder is gone, or with none remembered, asks where first and keeps the text`() {
        for (remembered in listOf("deleted-folder", null)) {
            val state = NewThreadFlowModel.open(SharedDraft("Look at this crash"), remembered, folders)
            assertEquals(FlowStep.WHERE, state.step)
            assertNull(state.folderId)
            assertTrue("Where says to choose", state.shared)
            assertEquals("Look at this crash", state.prompt)
            // Choosing a folder answers it.
            assertFalse(NewThreadFlowModel.choose(state, "orbit-api").shared)
        }
        // No folders at all yet (the PC can be browsed): still Where.
        assertEquals(FlowStep.WHERE, NewThreadFlowModel.open(SharedDraft("x"), "orbit-api", emptyList()).step)
    }

    @Test
    fun `the task, the pick and the mode stay as the steps change`() {
        val codex = RunChoice("codex", "gpt-5.4-mini", "low")
        var state = NewThreadFlowModel.choose(NewThreadFlowModel.open(null, null, folders), "orbit-api")
        state = NewThreadFlowModel.typed(state, "Fix the login redirect")
        state = NewThreadFlowModel.pick(state, codex)
        state = NewThreadFlowModel.pickMode(state, "codex", ThreadMode.PLAN)
        // Where, Browse, and back to What in the same folder: all of it is there.
        state = NewThreadFlowModel.back(state)!!
        state = NewThreadFlowModel.browse(state)
        state = NewThreadFlowModel.back(state)!!
        state = NewThreadFlowModel.choose(state, "orbit-api")
        assertEquals("Fix the login redirect", state.prompt)
        assertEquals(codex, state.chosen)
        val codexModes = listOf(ThreadMode.ASK, ThreadMode.AUTO, ThreadMode.FULL, ThreadMode.PLAN)
        assertEquals(ThreadMode.PLAN, NewThreadFlowModel.mode(state, "codex", codexModes, folder = null))
        // Plan only when the agent offers it.
        assertEquals(ThreadMode.ASK, NewThreadFlowModel.mode(state, "codex", codexModes - ThreadMode.PLAN, folder = null))
        // Another folder starts on its own first pick and its own mode; the task stays.
        state = NewThreadFlowModel.choose(NewThreadFlowModel.back(state)!!, "dashboard")
        assertNull(state.chosen)
        assertNull(state.mode)
        assertEquals("Fix the login redirect", state.prompt)
    }

    @Test
    fun `a folder that leaves the PC's list sends What back to Where, the task kept`() {
        var state = NewThreadFlowModel.typed(NewThreadFlowModel.choose(NewThreadFlowModel.open(null, null, folders), "dashboard"), "Ship it")
        assertEquals(state, NewThreadFlowModel.checked(state, folders))
        state = NewThreadFlowModel.checked(state, listOf("orbit-api"))
        assertEquals(FlowStep.WHERE, state.step)
        assertNull(state.folderId)
        assertEquals("Ship it", state.prompt)
    }

    @Test
    fun `Where's cards lead with the folder last started in, then by use, and stay put while open`() {
        val use = listOf(
            FolderUse("a", "alpha", usedMs = 100L),
            FolderUse("b", "beta", usedMs = 300L),
            FolderUse("c", "gamma", usedMs = 200L),
        )
        val opened = NewThreadFlowModel.order(use, first = "a", before = null)
        assertEquals(listOf("a", "b", "c"), opened)
        assertEquals(listOf("b", "c", "a"), NewThreadFlowModel.order(use, first = null, before = null))
        // A thread elsewhere makes "c" the newest: the shown order doesn't move under a finger.
        val later = use.map { if (it.id == "c") it.copy(usedMs = 900L) else it }
        assertEquals(opened, NewThreadFlowModel.order(later, first = "a", before = opened))
        // A folder added from Browse comes first; one removed on the PC leaves.
        val added = later.filter { it.id != "b" } + FolderUse("d", "delta", usedMs = 1_000L)
        assertEquals(listOf("d", "a", "c"), NewThreadFlowModel.order(added, first = "a", before = opened))
    }

    @Test
    fun `Start needs a ready agent, a folder, a task that fits, and every picture on the PC`() {
        assertTrue(NewThreadFlowModel.canStart(busy = false, agentReady = true, hasFolder = true, prompt = "Go", uploading = false, imagesTaken = true))
        assertFalse(NewThreadFlowModel.canStart(busy = false, agentReady = true, hasFolder = false, prompt = "Go", uploading = false, imagesTaken = true))
        assertFalse(NewThreadFlowModel.canStart(busy = false, agentReady = false, hasFolder = true, prompt = "Go", uploading = false, imagesTaken = true))
        assertFalse(NewThreadFlowModel.canStart(busy = false, agentReady = true, hasFolder = true, prompt = "Go", uploading = true, imagesTaken = true))
        assertFalse(NewThreadFlowModel.canStart(busy = false, agentReady = true, hasFolder = true, prompt = "Go", uploading = false, imagesTaken = false))
        assertFalse(NewThreadFlowModel.canStart(busy = true, agentReady = true, hasFolder = true, prompt = "Go", uploading = false, imagesTaken = true))
        assertFalse(NewThreadFlowModel.canStart(busy = false, agentReady = true, hasFolder = true, prompt = "x".repeat(AgentsModel.MAX_PROMPT_CHARS + 1), uploading = false, imagesTaken = true))
    }
}
