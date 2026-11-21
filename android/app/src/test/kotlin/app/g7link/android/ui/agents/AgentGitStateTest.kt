// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentDiffKind
import app.g7link.core.AgentDiffRow
import app.g7link.core.AgentGitWorktree
import app.g7link.core.AgentJob
import app.g7link.core.AgentJobKind
import app.g7link.core.AgentJobPhase
import app.g7link.core.AgentMode
import app.g7link.core.AgentPrBrief
import app.g7link.core.AgentPrState
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import app.g7link.core.Event
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What a PC's source-control events do to its agents' state on the phone (contract §SC6.1, §SC6.4). */
class AgentGitStateTest {
    private fun thread(branch: String? = null, changed: UInt? = null, worktree: AgentGitWorktree? = null) = AgentThread(
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
        branch = branch,
        changed = changed,
        worktree = worktree,
    )

    private fun job(phase: AgentJobPhase = AgentJobPhase.RUNNING, lines: List<String> = emptyList(), from: ULong = 0uL) = AgentJob(
        id = "0123456789abcdef",
        thread = "t1",
        kind = AgentJobKind.COMMIT,
        phase = phase,
        startedMs = 0L,
        endedMs = null,
        mine = true,
        lines = lines,
        from = from,
        dropped = 0u,
        result = null,
        error = null,
    )

    @Test
    fun `the strip comes from the thread until the PC says more`() {
        var state = AgentsModel.reduce(AgentsState(), Event.AgentThreadChanged("pc1", thread(branch = "feat/sync-retry", changed = 4u))).state
        assertEquals(GitStrip(branch = "feat/sync-retry", changed = 4), state.gitStrip("t1"))
        val pr = AgentPrBrief(212u, AgentPrState.OPEN, false, 7u, 0u, 0u)
        state = AgentsModel.reduce(state, Event.AgentGit("pc1", "t1", "feat/sync-retry", false, 4u, 2u, 0u, pr)).state
        val strip = state.gitStrip("t1")!!
        assertEquals(2, strip.ahead)
        assertEquals(PrBrief(212, PrState.OPEN, false, 7, 0, 0), strip.pr)
        // The thread's own count moves the strip the PC made.
        state = AgentsModel.reduce(state, Event.AgentThreadChanged("pc1", thread(branch = "feat/sync-retry", changed = 1u))).state
        assertEquals(1, state.gitStrip("t1")?.changed)
        assertEquals(2, state.gitStrip("t1")?.ahead)
        // Deleted: its strip goes with it.
        state = AgentsModel.reduce(state, Event.AgentThreadRemoved("pc1", "t1")).state
        assertNull(state.gitStrips["t1"])
    }

    @Test
    fun `a thread that says nothing of git shows no strip`() {
        val state = AgentsModel.reduce(AgentsState(), Event.AgentThreadChanged("pc1", thread())).state
        assertFalse(AgentGit.stripShown(state.gitStrip("t1")))
    }

    @Test
    fun `a worktree thread's strip carries its label`() {
        val state = AgentsModel.reduce(AgentsState(), Event.AgentThreadChanged("pc1", thread(branch = "g7/try-ack", worktree = AgentGitWorktree("try-ack", "g7/try-ack")))).state
        assertEquals("try-ack", state.gitStrip("t1")?.worktree)
    }

    @Test
    fun `a job's lines arrive in order and are kept with the PC's state`() {
        var state = AgentsModel.reduce(AgentsState(), Event.AgentGitJob("pc1", job(lines = listOf("a", "b")))).state
        state = AgentsModel.reduce(state, Event.AgentGitJob("pc1", job(lines = listOf("c"), from = 2uL))).state
        state = AgentsModel.reduce(state, Event.AgentGitJob("pc1", job(phase = AgentJobPhase.DONE, from = 3uL))).state
        val kept = state.gitJobs["0123456789abcdef"]!!
        assertEquals(listOf("a", "b", "c"), kept.lines)
        assertEquals(JobPhase.DONE, kept.phase)
        assertEquals("pc1", AgentsModel.pcOf(Event.AgentGitJob("pc1", job())))
        assertEquals("pc1", AgentsModel.pcOf(Event.AgentGit("pc1", "t1", null, true, 0u, null, null, null)))
    }

    @Test
    fun `a detached HEAD keeps the short id a status gave`() {
        val status = GitStatus(repo = RepoKind.OK, detached = "db48c89")
        var state = AgentsModel.withGitStatus(AgentsState(), "t1", status)
        assertEquals("db48c89", state.gitStrip("t1")?.detached)
        state = AgentsModel.reduce(state, Event.AgentGit("pc1", "t1", null, true, 0u, null, null, null)).state
        assertEquals("db48c89", state.gitStrip("t1")?.detached)
        state = AgentsModel.reduce(state, Event.AgentGit("pc1", "t1", "main", false, 0u, null, null, null)).state
        assertEquals("main", state.gitStrip("t1")?.branch)
        assertNull(state.gitStrip("t1")?.detached)
        assertNull("not a repository: no strip", AgentGitStrips.fromStatus(null, GitStatus(RepoKind.NONE)))
    }

    // ---- Comment on a line ----

    private fun row(kind: AgentDiffKind, old: UInt, new: UInt, text: String) = AgentDiffRow(kind, old, new, text, emptyList())

    @Test
    fun `a long-press picks a line, a tap picks the lines between, and the quote numbers them as the file does`() {
        val items = listOf(
            DiffItem.Hunk("@@ -40,4 +40,6 @@"),
            DiffItem.Line(row(AgentDiffKind.CONTEXT, 40u, 40u, "pub fn drain(&mut self)"), numbers = true),
            DiffItem.Line(row(AgentDiffKind.DEL, 41u, 0u, "    link.send_all(&rows)?;"), numbers = true),
            DiffItem.Line(row(AgentDiffKind.ADD, 0u, 41u, "    while let Some(row) = self.rows.front() {"), numbers = true),
            DiffItem.Line(row(AgentDiffKind.ADD, 0u, 42u, "        link.send(row)?;"), numbers = true),
        )
        assertNull("a hunk's head isn't a line", AgentDiffLines.start(items, 0))
        var picked = AgentDiffLines.start(items, 4)
        picked = AgentDiffLines.tap(items, picked, 3)
        assertEquals(3..4, picked?.range)
        val quote = AgentQuotes.of("src/sync/queue.rs", AgentDiffLines.quoted(items, picked))!!
        assertEquals("src/sync/queue.rs:41–42", AgentQuotes.label(quote))
        assertEquals(2, AgentDiffLines.count(items, picked))
        // A removed line counts by its old number.
        assertEquals(listOf(QuotedLine(41, "    link.send_all(&rows)?;")), AgentDiffLines.quoted(items, LineSelection(2, 2)))
        // Tapping the one line picked clears it.
        assertNull(AgentDiffLines.tap(items, LineSelection(4, 4), 4))
    }

    // ---- The pull request's base, and links ----

    @Test
    fun `a pull request goes into a branch on the push remote, by its name there`() {
        val branches = listOf(
            GitBranch("main"),
            GitBranch("origin/main", remote = "origin"),
            GitBranch("origin/HEAD", remote = "origin"),
            GitBranch("origin/feat/sync-retry", remote = "origin"),
            GitBranch("origin/release", remote = "origin"),
            GitBranch("fork/main", remote = "fork"),
        )
        assertEquals(listOf("main", "release"), AgentPrs.bases(branches, "origin", "feat/sync-retry"))
        assertTrue(AgentPrs.bases(branches, null, "feat/sync-retry").isEmpty())
    }

    @Test
    fun `a list row's link opens when it's https, whatever host the PC let through`() {
        assertTrue(AgentGit.openable("https://git.example.org/a/b/pull/3", host = null))
        assertFalse(AgentGit.openable("http://git.example.org/a/b/pull/3", host = null))
        assertFalse(AgentGit.openable("https:///nohost", host = null))
    }
}
