// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentAct
import app.g7link.core.AgentActs
import app.g7link.core.AgentDraft
import app.g7link.core.AgentDraftSaved
import app.g7link.core.AgentImage
import app.g7link.core.AgentRewindBlocked
import app.g7link.core.AgentRewindPlan
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The message sheet, the rewind sheet (conversation only from the phone) and the draft on the PC. */
class AgentRewindModelTest {
    private fun acts(fork: AgentAct = AgentAct.ON, rewind: AgentAct = AgentAct.ON, edit: AgentAct = AgentAct.ON) =
        AgentActs(fork = fork, rewind = rewind, edit = edit, restore = AgentAct.HIDDEN)

    private fun plan(blocked: AgentRewindBlocked? = null, expiresMs: Long = 120_000L) =
        AgentRewindPlan(token = "0123456789abcdef0123456789abcdef", expiresMs = expiresMs, turns = 2u, messages = 3u, blocked = blocked)

    private fun draft(text: String, rev: ULong, by: String? = "guchu") = AgentDraft(key = "t1", text = text, images = emptyList(), rev = rev, byName = by)

    // ---- The message sheet (board 20) ----

    @Test
    fun `your message offers copy, edit, rewind and fork, the final answer copy and fork`() {
        assertEquals(
            listOf(MessageAction.COPY, MessageAction.EDIT, MessageAction.REWIND, MessageAction.FORK),
            AgentMessageActions.rows(acts(), user = true, offline = false).map { it.action },
        )
        assertEquals(
            listOf(MessageAction.COPY, MessageAction.FORK),
            AgentMessageActions.rows(acts(rewind = AgentAct.HIDDEN, edit = AgentAct.HIDDEN), user = false, offline = false).map { it.action },
        )
        assertTrue(AgentMessageActions.rows(acts(), user = true, offline = false).all { it.block == null })
    }

    @Test
    fun `while the thread works the render crate hides them all, and Copy stays`() {
        assertEquals(listOf(MessageActionRow(MessageAction.COPY)), AgentMessageActions.rows(null, user = true, offline = false))
        val hidden = acts(fork = AgentAct.HIDDEN, rewind = AgentAct.HIDDEN, edit = AgentAct.HIDDEN)
        assertEquals(listOf(MessageAction.COPY), AgentMessageActions.rows(hidden, user = true, offline = false).map { it.action })
    }

    @Test
    fun `an action that can't be done shows off with why, and everything but Copy needs the PC`() {
        val rows = AgentMessageActions.rows(acts(fork = AgentAct.LATEST_ONLY, rewind = AgentAct.LATEST_ONLY, edit = AgentAct.LATEST_ONLY), user = true, offline = false)
        assertEquals(listOf(null, ActBlock.LatestOnly, ActBlock.LatestOnly, ActBlock.LatestOnly), rows.map { it.block })
        val noPoint = AgentMessageActions.rows(acts(rewind = AgentAct.NO_RESTORE_POINT), user = true, offline = false)
        assertEquals(ActBlock.NoRestorePoint, noPoint.first { it.action == MessageAction.REWIND }.block)
        val offline = AgentMessageActions.rows(acts(), user = true, offline = true)
        assertNull(offline.first().block)
        assertTrue(offline.drop(1).all { it.block == ActBlock.NeedsPc })
        assertEquals("its own reason first", ActBlock.NotGit, AgentMessageActions.rows(acts(rewind = AgentAct.NOT_GIT), user = true, offline = true)[2].block)
    }

    // ---- The rewind sheet (board 21) ----

    @Test
    fun `the sheet loads its plan, and Rewind waits for one nothing blocks`() {
        var sheet: RewindSheet? = AgentRewind.opened(3u, refill = false)
        assertFalse(AgentRewind.canApply(sheet, 0L))
        assertSame("another turn's plan is not this sheet's", sheet, AgentRewind.previewed(sheet, 2u, plan()))
        sheet = AgentRewind.previewed(sheet, 3u, plan())
        assertTrue(sheet is RewindSheet.Ready)
        assertTrue(AgentRewind.canApply(sheet, 1_000L))
        assertFalse("expired", AgentRewind.canApply(sheet, 120_000L))
        val blocked = AgentRewind.previewed(AgentRewind.opened(3u, false), 3u, plan(AgentRewindBlocked.AGENT))
        assertFalse(AgentRewind.canApply(blocked, 1_000L))
        assertSame("a blocked plan isn't applied", blocked, AgentRewind.applying(blocked))
    }

    @Test
    fun `applying, then a used or expired plan is asked for again and anything else says why`() {
        val ready = AgentRewind.previewed(AgentRewind.opened(3u, refill = true), 3u, plan())
        val applying = AgentRewind.applying(ready)
        assertTrue(applying is RewindSheet.Applying)
        assertEquals("edit from here keeps its refill", true, applying!!.refill)
        assertEquals(RewindSheet.Loading(3u, true), AgentRewind.applyFailed(applying, AgentFailure.NOT_FOUND))
        assertEquals(RewindSheet.Failed(3u, true, AgentFailure.OTHER), AgentRewind.applyFailed(applying, AgentFailure.OTHER))
        assertSame(ready, AgentRewind.applyFailed(ready, AgentFailure.OTHER))
        assertEquals(RewindSheet.Failed(3u, true, AgentFailure.TIMEOUT), AgentRewind.previewFailed(AgentRewind.opened(3u, true), 3u, AgentFailure.TIMEOUT))
        assertSame("a failure doesn't replace an apply on its way", applying, AgentRewind.previewFailed(applying, 3u, AgentFailure.TIMEOUT))
    }

    @Test
    fun `every reason the PC gives has its words`() {
        assertNull(AgentRewind.reason(null))
        val all = AgentRewindBlocked.entries.map { AgentRewind.reason(it) }
        assertEquals(AgentRewindBlocked.entries.size, all.filterNotNull().distinct().size)
        assertEquals(AgentRewind.Reason.LATEST_ONLY, AgentRewind.reason(AgentRewindBlocked.AGENT))
        assertEquals(AgentRewind.Reason.BUSY, AgentRewind.reason(AgentRewindBlocked.BUSY))
    }

    // ---- The draft (phase 2 §Q6.1, phase 3 §U9.3) ----

    @Test
    fun `an empty composer takes the stored draft, one with words is offered it`() {
        val taken = AgentDrafts.loaded(DraftState(), draft("Sawa. Sasa ongeza", 4uL), "")
        assertEquals("Sawa. Sasa ongeza", taken.text)
        assertEquals(4uL, taken.state.rev)
        assertTrue(taken.state.loaded)
        assertNull(taken.state.offer)
        val offered = AgentDrafts.loaded(DraftState(), draft("Sawa", 4uL), "nikiandika")
        assertNull(offered.text)
        assertEquals("Sawa", offered.state.offer?.text)
        val none = AgentDrafts.loaded(DraftState(), null, "")
        assertTrue(none.state.loaded)
        assertEquals(0uL, none.state.rev)
        val images = AgentDrafts.loaded(DraftState(), AgentDraft("t1", "", listOf(AgentImage("img1", "a.png", 10u, 1u, 1u)), 2uL, null), "")
        assertEquals(listOf("img1"), images.images)
    }

    @Test
    fun `it's written only after an edit, never while a question waits, and never before it was read`() {
        val read = AgentDrafts.loaded(DraftState(), null, "").state
        assertFalse(AgentDrafts.mayWrite(read, questionWaiting = false))
        val edited = AgentDrafts.edited(read)
        assertTrue(AgentDrafts.mayWrite(edited, questionWaiting = false))
        assertFalse("no answer travels as a draft", AgentDrafts.mayWrite(edited, questionWaiting = true))
        assertFalse(AgentDrafts.mayWrite(AgentDrafts.edited(DraftState()), questionWaiting = false))
        assertEquals(500L, AgentDrafts.SAVE_AFTER_MS)
    }

    @Test
    fun `written, its rev moves on, and another device's newer one is offered`() {
        val edited = AgentDrafts.edited(AgentDrafts.loaded(DraftState(), draft("a", 1uL), "a").state)
        val saved = AgentDrafts.saved(edited, AgentDraftSaved(rev = 2uL, conflict = null), "ab")
        assertEquals(2uL, saved.rev)
        assertFalse(saved.dirty)
        val conflict = AgentDrafts.saved(edited, AgentDraftSaved(rev = 1uL, conflict = draft("from the PC", 3uL)), "ab")
        assertEquals("from the PC", conflict.offer?.text)
        assertEquals(3uL, conflict.rev)
        val same = AgentDrafts.saved(edited, AgentDraftSaved(rev = 1uL, conflict = draft("ab", 3uL)), "ab")
        assertNull("the same words: nothing to offer", same.offer)
    }

    @Test
    fun `a change elsewhere replaces the words while the box isn't focused and is offered while it is`() {
        val state = AgentDrafts.loaded(DraftState(), draft("a", 2uL), "a").state
        assertFalse("its own write", AgentDrafts.stale(state, 2uL))
        assertTrue(AgentDrafts.stale(state, 3uL))
        val unfocused = AgentDrafts.arrived(state, draft("from the PC", 3uL), focused = false, local = "a")
        assertEquals("from the PC", unfocused.text)
        assertEquals(3uL, unfocused.state.rev)
        val focused = AgentDrafts.arrived(state, draft("from the PC", 3uL), focused = true, local = "a")
        assertNull(focused.text)
        assertEquals("guchu", focused.state.offer?.byName)
        val used = AgentDrafts.used(focused.state)
        assertEquals("from the PC", used.text)
        assertNull(used.state.offer)
        assertFalse(used.state.dirty)
        val old = AgentDrafts.arrived(unfocused.state, draft("older", 2uL), focused = false, local = "x")
        assertNull("an older one changes nothing", old.text)
    }

    @Test
    fun `a draft sent or cleared elsewhere takes back its offer and is never offered`() {
        val state = AgentDrafts.loaded(DraftState(), draft("a", 2uL), "a").state
        val offered = AgentDrafts.arrived(state, draft("from the PC", 3uL), focused = true, local = "mine")
        assertEquals("from the PC", offered.state.offer?.text)
        val sent = AgentDrafts.arrived(offered.state, draft("", 4uL), focused = true, local = "mine")
        assertNull("sent on the PC: nothing left to offer", sent.state.offer)
        assertNull("the words typed here stay", sent.text)
        assertEquals(4uL, sent.state.rev)
        val idle = AgentDrafts.arrived(offered.state, draft("", 4uL), focused = false, local = "from the PC")
        assertEquals("an idle box follows the PC", "", idle.text)
        assertNull(AgentDrafts.loaded(DraftState(), draft("", 6uL), "mine").state.offer)
        val edited = AgentDrafts.edited(state)
        val conflict = AgentDrafts.saved(edited, AgentDraftSaved(rev = 2uL, conflict = draft("", 5uL)), "mine")
        assertNull(conflict.offer)
        assertEquals(5uL, conflict.rev)
        assertTrue("still to be written", conflict.dirty)
    }

    @Test
    fun `sending drops the draft with the message`() {
        val sent = AgentDrafts.sent(AgentDrafts.edited(AgentDrafts.loaded(DraftState(), draft("a", 5uL), "a").state))
        assertEquals(0uL, sent.rev)
        assertFalse(sent.dirty)
        assertNull(sent.offer)
    }
}
