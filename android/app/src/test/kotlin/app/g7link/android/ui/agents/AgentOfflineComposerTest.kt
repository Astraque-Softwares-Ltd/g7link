// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentDiffDoc
import app.g7link.core.AgentDiffFile
import app.g7link.core.AgentDiffNote
import app.g7link.core.AgentFileStatus
import app.g7link.core.AgentPatch
import app.g7link.core.AgentTurnFile
import app.g7link.core.AgentTurnFiles
import app.g7link.core.AgentTurnRange
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The composer while the PC can't be reached (contract §U9.3 "Offline"), and the diff screen's rules (§U9.1 "Changes"). */
class AgentOfflineComposerTest {
    @Test
    fun `offline, words wait on the phone with Send and the PC-is-back hint, no Stop, no pictures`() {
        val view = AgentDock.composer(ComposerInput(running = false, draft = "Pia andika changelog", offline = true))
        assertTrue(view.enabled)
        assertEquals(ComposerHint.OFFLINE, view.hint)
        assertEquals(ComposerAction.SEND, view.action)
        assertTrue(view.actionEnabled)
        assertFalse(view.attach)
        assertFalse(view.showStop)
        assertEquals(SettingsAccess.INERT, view.settings)
    }

    @Test
    fun `offline, a working thread's message waits too, and nothing is stopped or answered from here`() {
        val working = AgentDock.composer(ComposerInput(running = true, draft = "na", offline = true, followUp = FollowUp.STEER))
        assertEquals(ComposerAction.SEND, working.action)
        assertFalse(working.showStop)
        val question = AgentDock.composer(ComposerInput(running = true, draft = "x", offline = true, question = QuestionField.OTHER))
        assertEquals(ComposerHint.OFFLINE, question.hint)
    }

    @Test
    fun `offline, pictures can't wait, an empty box can't send, and nothing goes without a thread`() {
        assertFalse(AgentDock.composer(ComposerInput(running = false, draft = "a", images = 1, offline = true)).actionEnabled)
        assertFalse(AgentDock.composer(ComposerInput(running = false, draft = "  ", offline = true)).actionEnabled)
        val noThread = AgentDock.composer(ComposerInput(running = false, hasThread = false, draft = "a", offline = true))
        assertFalse(noThread.enabled)
        assertFalse(noThread.actionEnabled)
        assertFalse(AgentDock.composer(ComposerInput(running = false, draft = "a", sending = true, offline = true)).actionEnabled)
    }

    @Test
    fun `online, the composer is as before`() {
        val view = AgentDock.composer(ComposerInput(running = false, draft = "a"))
        assertEquals(ComposerHint.MESSAGE, view.hint)
        assertTrue(view.actionEnabled)
    }

    // ---- The diff screen ----

    private fun file(path: String, status: AgentFileStatus?, binary: Boolean = false) =
        AgentTurnFile(path = path, status = status, from = null, adds = 1u, dels = 1u, binary = binary)

    @Test
    fun `this turn is the one, the whole thread runs from the first turn to the newest`() {
        assertEquals(AgentTurnRange.Turn(3u), AgentDiffModel.range(DiffScope.THIS_TURN, 3u, 7u))
        assertEquals(AgentTurnRange.Span(1u, 7u), AgentDiffModel.range(DiffScope.WHOLE_THREAD, 3u, 7u))
        assertEquals("a thread whose newest isn't known yet", AgentTurnRange.Span(1u, 3u), AgentDiffModel.range(DiffScope.WHOLE_THREAD, 3u, 0u))
    }

    @Test
    fun `files too large to record come after the others, and a file's letter is git's`() {
        val (shown, unrecorded) = AgentDiffModel.split(listOf(file("a", AgentFileStatus.MODIFIED), file("big.bin", AgentFileStatus.UNRECORDED), file("b", AgentFileStatus.ADDED)))
        assertEquals(listOf("a", "b"), shown.map { it.path })
        assertEquals(listOf("big.bin"), unrecorded.map { it.path })
        assertEquals("A", AgentDiffModel.letter(AgentFileStatus.ADDED))
        assertEquals("D", AgentDiffModel.letter(AgentFileStatus.DELETED))
        assertEquals("R", AgentDiffModel.letter(AgentFileStatus.RENAMED))
        assertEquals("T", AgentDiffModel.letter(AgentFileStatus.TYPE_CHANGED))
        assertEquals("a word this build doesn't know reads as changed", "M", AgentDiffModel.letter(null))
    }

    @Test
    fun `past 500 files the rest are counted for one turn only`() {
        val listed = AgentTurnFiles(files = (1..500).map { file("f$it", AgentFileStatus.MODIFIED) }, more = true, adds = 0u, dels = 0u)
        assertEquals(120, AgentDiffModel.more(DiffScope.THIS_TURN, listed, 620))
        assertEquals(0, AgentDiffModel.more(DiffScope.WHOLE_THREAD, listed, 620))
        assertEquals(0, AgentDiffModel.more(DiffScope.THIS_TURN, listed, null))
        assertEquals(0, AgentDiffModel.more(DiffScope.THIS_TURN, listed.copy(more = false), 620))
    }

    private fun doc(note: AgentDiffNote? = null, more: Boolean = false) = AgentDiffDoc(
        files = listOf(AgentDiffFile(path = "a", adds = 1u, dels = 0u, rows = emptyList(), note = note)),
        adds = 1u,
        dels = 0u,
        more = more,
    )

    @Test
    fun `a patch shows binary, or its rows cut short when the PC or the parser says so`() {
        val patch = AgentPatch(patch = "--- a\n+++ b\n", truncated = false, binary = false)
        assertEquals(FileDiff.Binary, AgentDiffModel.shown(null, patch.copy(binary = true), null))
        assertEquals(FileDiff.Binary, AgentDiffModel.shown(file("a", AgentFileStatus.MODIFIED, binary = true), patch, doc()))
        assertEquals(FileDiff.Binary, AgentDiffModel.shown(null, patch, doc(AgentDiffNote.BINARY)))
        assertEquals(FileDiff.Shown(doc(), cut = false), AgentDiffModel.shown(null, patch, doc()))
        assertEquals(true, (AgentDiffModel.shown(null, patch.copy(truncated = true), doc()) as FileDiff.Shown).cut)
        assertEquals(true, (AgentDiffModel.shown(null, patch, doc(AgentDiffNote.TRUNCATED)) as FileDiff.Shown).cut)
        assertTrue(AgentDiffModel.shown(null, patch, null) is FileDiff.Failed)
    }
}
