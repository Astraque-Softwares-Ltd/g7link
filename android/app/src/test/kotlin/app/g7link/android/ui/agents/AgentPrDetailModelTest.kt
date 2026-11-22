// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A pull request read in full: what the view shows and when "Ask <agent> to review it" is offered (phase 6 §X6.6). */
class AgentPrDetailModelTest {
    private val pr = PrInfo(
        number = 212,
        title = "Stock sync: keep rows until the server acknowledges",
        state = PrState.OPEN,
        draft = false,
        url = "https://github.com/example/duka-pos/pull/212",
        base = "main",
        head = "feat/sync-retry",
    )

    private fun detail(pr: PrInfo? = this.pr, gh: GhState = GhState.OK, why: PrDetailWhy? = null, threadId: String? = null) = PrDetail(
        pr = pr,
        gh = gh,
        why = why,
        repo = "example/duka-pos",
        author = "octocat",
        changed = 3,
        files = listOf(GitFile("src/sync.rs", change = FileChange.MODIFIED, adds = 12, dels = 3)),
        threadId = threadId,
    )

    // ---- The view's states ----

    @Test
    fun `a pull request is shown, with why it may be an older read`() {
        assertEquals(PrDetailView.Shown(detail(), unreached = false, tooLarge = false), AgentPrDetails.view(AgentCall.Ok(detail()), offline = false))
        val old = AgentPrDetails.view(AgentCall.Ok(detail(why = PrDetailWhy.NETWORK)), offline = false) as PrDetailView.Shown
        assertTrue(old.unreached)
        val large = AgentPrDetails.view(AgentCall.Ok(detail(why = PrDetailWhy.TOO_LARGE)), offline = false) as PrDetailView.Shown
        assertTrue(large.tooLarge)
        assertFalse(large.unreached)
    }

    @Test
    fun `without a pull request it says the GitHub CLI first, then why there's none`() {
        assertEquals(PrDetailView.Gh(GhState.SIGNED_OUT), AgentPrDetails.view(AgentCall.Ok(detail(pr = null, gh = GhState.SIGNED_OUT, why = PrDetailWhy.NO_PR)), offline = false))
        assertEquals(PrDetailView.Missing(PrDetailWhy.NOT_GITHUB), AgentPrDetails.view(AgentCall.Ok(detail(pr = null, why = PrDetailWhy.NOT_GITHUB)), offline = false))
        assertEquals("no word: none", PrDetailView.Missing(PrDetailWhy.NO_PR), AgentPrDetails.view(AgentCall.Ok(detail(pr = null)), offline = false))
        assertEquals(PrDetailView.Missing(PrDetailWhy.TOO_LARGE), AgentPrDetails.view(AgentCall.Ok(detail(pr = null, why = PrDetailWhy.TOO_LARGE)), offline = false))
    }

    @Test
    fun `before an answer it waits, or needs the PC`() {
        assertEquals(PrDetailView.Loading, AgentPrDetails.view(null, offline = false))
        assertEquals(PrDetailView.NeedsPc, AgentPrDetails.view(null, offline = true))
        assertEquals(PrDetailView.NeedsPc, AgentPrDetails.view(AgentCall.Failed(AgentFailure.OFFLINE), offline = false))
        assertEquals(PrDetailView.Failed, AgentPrDetails.view(AgentCall.Failed(AgentFailure.TIMEOUT), offline = false))
        assertTrue("what was read stays while the PC is away", AgentPrDetails.view(AgentCall.Ok(detail()), offline = true) is PrDetailView.Shown)
    }

    // ---- "Ask <agent> to review it" ----

    @Test
    fun `the button needs a thread, the one it was opened from, else the one the PC found`() {
        assertEquals(PrAsk.Ready("t1", 212), AgentPrDetails.ask(detail(), threadId = "t1", readonly = false, busy = false, offline = false, asking = false))
        assertEquals(PrAsk.Ready("t9", 212), AgentPrDetails.ask(detail(threadId = "t9"), threadId = null, readonly = false, busy = false, offline = false, asking = false))
        assertEquals("opened from a thread, that thread", PrAsk.Ready("t1", 212), AgentPrDetails.ask(detail(threadId = "t9"), threadId = "t1", readonly = false, busy = false, offline = false, asking = false))
        assertEquals(PrAsk.NoThread, AgentPrDetails.ask(detail(), threadId = null, readonly = false, busy = false, offline = false, asking = false))
        assertEquals("a read-only thread can't be asked", PrAsk.NoThread, AgentPrDetails.ask(detail(), threadId = "t1", readonly = true, busy = false, offline = false, asking = false))
        assertEquals(PrAsk.NoThread, AgentPrDetails.ask(detail(pr = null), threadId = "t1", readonly = false, busy = false, offline = false, asking = false))
    }

    @Test
    fun `the button is off while the PC is away, the thread works, or an ask is on its way`() {
        assertEquals(PrAsk.NeedsPc, AgentPrDetails.ask(detail(), threadId = "t1", readonly = false, busy = true, offline = true, asking = false))
        assertEquals(PrAsk.Busy, AgentPrDetails.ask(detail(), threadId = "t1", readonly = false, busy = true, offline = false, asking = false))
        assertEquals(PrAsk.Asking, AgentPrDetails.ask(detail(), threadId = "t1", readonly = false, busy = false, offline = false, asking = true))
    }

    // ---- Words from GitHub ----

    @Test
    fun `a file's change and a review's word as the PC lower-cased them`() {
        assertEquals(FileChange.ADDED, AgentPrDetails.change("added"))
        assertEquals(FileChange.ADDED, AgentPrDetails.change("copied"))
        assertEquals(FileChange.DELETED, AgentPrDetails.change("deleted"))
        assertEquals(FileChange.RENAMED, AgentPrDetails.change("renamed"))
        assertEquals(FileChange.MODIFIED, AgentPrDetails.change("modified"))
        assertEquals("a word this phone doesn't know", FileChange.MODIFIED, AgentPrDetails.change("changed"))
        assertEquals(PrReviewWord.APPROVED, AgentPrDetails.review("approved"))
        assertEquals(PrReviewWord.CHANGES_REQUESTED, AgentPrDetails.review("changes_requested"))
        assertEquals(PrReviewWord.COMMENTED, AgentPrDetails.review("commented"))
        assertEquals(PrReviewWord.DISMISSED, AgentPrDetails.review("dismissed"))
        assertNull(AgentPrDetails.review("pending"))
        assertNull(AgentPrDetails.review(null))
    }

    @Test
    fun `files past those listed, comments oldest first, and the host of a repository`() {
        assertEquals(0, AgentPrDetails.moreFiles(detail()))
        assertEquals(2, AgentPrDetails.moreFiles(detail().copy(filesMore = true)))
        assertTrue(AgentPrDetails.noComments(detail()))
        assertFalse("older ones left out aren't none", AgentPrDetails.noComments(detail().copy(commentsOlder = 3)))
        val ordered = AgentPrDetails.ordered(listOf(PrComment("b", 20L, "later"), PrComment("a", 10L, "first"), PrComment("c", 20L, "tie")))
        assertEquals(listOf("first", "later", "tie"), ordered.map { it.body })
        assertEquals("github.com", AgentPrDetails.host("example/duka-pos"))
        assertEquals("git.example.com", AgentPrDetails.host("git.example.com/team/app"))
        assertTrue(AgentPrDetails.opens(detail(), "src/sync.rs"))
        assertFalse("only a file the PC listed", AgentPrDetails.opens(detail(), "../etc/passwd"))
    }
}
