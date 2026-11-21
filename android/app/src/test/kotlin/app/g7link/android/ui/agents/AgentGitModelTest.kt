// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The strip, the sheet's rows, branches, jobs and "Ask <agent> to write it" (contract §SC9.1, §SC9.3). */
class AgentGitModelTest {
    private val files = listOf(GitFile("src/a.rs", change = FileChange.MODIFIED, adds = 3, dels = 1), GitFile("docs/", change = FileChange.UNTRACKED, folder = true))

    private fun status(
        branch: String? = "feat/sync-retry",
        upstream: String? = "origin/feat/sync-retry",
        ahead: Int? = 2,
        behind: Int? = 0,
        changed: List<GitFile> = files,
        remotes: List<String> = listOf("origin"),
        defaultBranch: String? = "main",
    ) = GitStatus(
        repo = RepoKind.OK,
        branch = branch,
        upstream = upstream,
        ahead = ahead,
        behind = behind,
        remotes = remotes,
        remote = upstream?.substringBefore('/'),
        defaultBranch = defaultBranch,
        onDefault = branch != null && branch == defaultBranch,
        files = changed,
        total = changed.size,
        adds = 90,
        dels = 17,
    )

    private fun pr(pass: Int = 7, fail: Int = 0, pending: Int = 0, checks: List<PrCheck> = emptyList()) = PrInfo(
        number = 212,
        title = "Stock sync: keep rows until the server acknowledges",
        state = PrState.OPEN,
        draft = false,
        url = "https://github.com/example/duka-pos/pull/212",
        base = "main",
        head = "feat/sync-retry",
        checks = checks,
        pass = pass,
        fail = fail,
        pending = pending,
    )

    // ---- The push row's three states ----

    @Test
    fun `push is off with the switch's name until the PC allows this phone`() {
        assertEquals(PushRow.SwitchOff, AgentGit.pushRow(status(), offline = false, pushAllowed = false))
    }

    @Test
    fun `push says what goes where once allowed`() {
        val row = AgentGit.pushRow(status(), offline = false, pushAllowed = true) as PushRow.Ready
        assertEquals("feat/sync-retry", row.branch)
        assertEquals("origin", row.remote)
        assertEquals(2, row.commits)
        assertFalse(row.toDefault)
    }

    @Test
    fun `a push to the default branch says so in its confirmation`() {
        val row = AgentGit.pushRow(status(branch = "main", upstream = "origin/main", ahead = 1), offline = false, pushAllowed = true) as PushRow.Ready
        assertTrue(row.toDefault)
        assertEquals(1, row.commits)
    }

    @Test
    fun `what can't be pushed says why before the switch does`() {
        assertEquals(PushRow.NeedsPc, AgentGit.pushRow(status(), offline = true, pushAllowed = true))
        assertEquals(PushRow.UpToDate, AgentGit.pushRow(status(ahead = 0), offline = false, pushAllowed = false))
        assertEquals(PushRow.NoRemote, AgentGit.pushRow(status(upstream = null, remotes = emptyList()), offline = false, pushAllowed = true))
        assertEquals(PushRow.Detached, AgentGit.pushRow(status(branch = null).copy(detached = "db48c89"), offline = false, pushAllowed = true))
        assertEquals(PushRow.NoCommits, AgentGit.pushRow(status().copy(unborn = true), offline = false, pushAllowed = true))
    }

    @Test
    fun `a branch not on a remote yet pushes with no count, and the remote is picked when git can't tell`() {
        val fresh = AgentGit.pushRow(status(upstream = null, ahead = null), offline = false, pushAllowed = true) as PushRow.Ready
        assertNull(fresh.commits)
        assertEquals("origin", fresh.remote)
        val two = AgentGit.pushRow(status(upstream = null, ahead = null, remotes = listOf("fork", "upstream")), offline = false, pushAllowed = true) as PushRow.Ready
        assertNull(two.remote)
        assertEquals(listOf("fork", "upstream"), two.remotes)
        val one = AgentGit.pushRow(status(upstream = null, ahead = null, remotes = listOf("fork")), offline = false, pushAllowed = true) as PushRow.Ready
        assertEquals("fork", one.remote)
    }

    // ---- The other rows ----

    @Test
    fun `review and commit is off with nothing changed, while git or the thread is busy, and offline`() {
        assertEquals(ReviewRow.Ready(2, 90, 17), AgentGit.reviewRow(status(), offline = false, threadBusy = false))
        assertEquals(ReviewRow.Nothing, AgentGit.reviewRow(status(changed = emptyList()).copy(total = 0), offline = false, threadBusy = false))
        assertEquals(ReviewRow.ThreadBusy, AgentGit.reviewRow(status(), offline = false, threadBusy = true))
        assertEquals(ReviewRow.NeedsPc, AgentGit.reviewRow(status(), offline = true, threadBusy = false))
        assertEquals(ReviewRow.Blocked(GitStateLine.Busy), AgentGit.reviewRow(status().copy(locked = true), offline = false, threadBusy = false))
        assertEquals(ReviewRow.Blocked(GitStateLine.Doing(GitDoing.MERGE)), AgentGit.reviewRow(status().copy(doing = GitDoing.MERGE), offline = false, threadBusy = false))
        assertEquals(ReviewRow.Blocked(GitStateLine.Conflicts(1)), AgentGit.reviewRow(status().copy(conflicts = 1), offline = false, threadBusy = false))
        assertEquals(ReviewRow.Blocked(GitStateLine.Detached("db48c89")), AgentGit.reviewRow(status(branch = null).copy(detached = "db48c89"), offline = false, threadBusy = false))
        // A first commit is fine.
        assertTrue(AgentGit.reviewRow(status().copy(unborn = true), offline = false, threadBusy = false) is ReviewRow.Ready)
    }

    @Test
    fun `the pull request row shows the branch's one, else what stops one`() {
        val shown = PrView(pr = pr(), gh = GhState.OK, repo = "example/duka-pos")
        assertEquals(PrRowState.Shown(pr(), unreached = false), AgentGit.prRow(status(), shown, offline = false, pushAllowed = false))
        assertEquals(PrRowState.Shown(pr(), unreached = true), AgentGit.prRow(status(), shown.copy(why = NoPr.NETWORK), offline = false, pushAllowed = false))
        assertEquals(PrRowState.Loading, AgentGit.prRow(status(), null, offline = false, pushAllowed = true))
        assertEquals(PrRowState.Gh(GhState.MISSING), AgentGit.prRow(status(), PrView(null, GhState.MISSING), offline = false, pushAllowed = true))
        assertEquals(PrRowState.NotGithub, AgentGit.prRow(status(), PrView(null, GhState.OK, why = NoPr.NOT_GITHUB), offline = false, pushAllowed = true))
        assertEquals(PrRowState.NotPushed, AgentGit.prRow(status(), PrView(null, GhState.OK), offline = false, pushAllowed = true))
        assertEquals(PrRowState.SwitchOff, AgentGit.prRow(status(ahead = 0), PrView(null, GhState.OK), offline = false, pushAllowed = false))
        assertEquals(PrRowState.CanOpen, AgentGit.prRow(status(ahead = 0), PrView(null, GhState.OK), offline = false, pushAllowed = true))
        assertEquals(PrRowState.NeedsPc, AgentGit.prRow(status(), shown, offline = true, pushAllowed = true))
    }

    @Test
    fun `a worktree thread's branch is its worktree`() {
        assertEquals(BranchRowState.InWorktree, AgentGit.branchRow(status().copy(worktree = "try-ack"), offline = false))
        assertEquals(BranchRowState.Ready("feat/sync-retry", null), AgentGit.branchRow(status(), offline = false))
    }

    @Test
    fun `the state line names what's in the way, most pressing first`() {
        assertEquals(GitStateLine.NotRepo, AgentGit.stateLine(GitStatus(RepoKind.NONE)))
        assertEquals(GitStateLine.NoGit, AgentGit.stateLine(GitStatus(RepoKind.NO_GIT)))
        assertEquals(GitStateLine.Refused, AgentGit.stateLine(GitStatus(RepoKind.REFUSED)))
        assertEquals(GitStateLine.Busy, AgentGit.stateLine(status().copy(locked = true, doing = GitDoing.REBASE)))
        assertEquals(GitStateLine.NoCommits, AgentGit.stateLine(status(changed = emptyList()).copy(unborn = true, total = 0)))
        assertNull(AgentGit.stateLine(status()))
        assertTrue(AgentGit.inStep(status(ahead = 0, changed = emptyList()).copy(total = 0)))
        assertFalse(AgentGit.inStep(status(ahead = 0)))
    }

    // ---- Checks ----

    @Test
    fun `checks read failing first, then running, and skipped ones aren't counted`() {
        assertEquals(ChecksLine.Passed(7), AgentGit.checksLine(pr()))
        assertEquals(ChecksLine.Failed(2, 7), AgentGit.checksLine(pr(pass = 5, fail = 2)))
        assertEquals(ChecksLine.Running(3, 7), AgentGit.checksLine(pr(pass = 4, pending = 3)))
        assertEquals(ChecksLine.None, AgentGit.checksLine(pr(pass = 0)))
        val listed = listOf(
            PrCheck("build", CheckState.PASS),
            PrCheck("lint", CheckState.SKIPPED),
            PrCheck("e2e", CheckState.FAIL),
            PrCheck("docs", CheckState.CANCELLED),
        )
        assertEquals(ChecksLine.Failed(1, 3), AgentGit.checksLine(pr(pass = 1, fail = 1, checks = listed)))
        assertEquals(listOf("e2e", "build", "lint", "docs"), AgentGit.sortedChecks(listed).map { it.name })
        assertEquals(CheckState.FAIL, AgentGit.checksMark(5, 1, 1))
        assertEquals(CheckState.PENDING, AgentGit.checksMark(5, 0, 1))
        assertNull(AgentGit.checksMark(0, 0, 0))
    }

    @Test
    fun `only an https link on the pull request's host opens`() {
        assertTrue(AgentGit.openable("https://github.com/example/duka-pos/pull/212"))
        assertFalse(AgentGit.openable("http://github.com/example/duka-pos/pull/212"))
        assertFalse(AgentGit.openable("https://github.com.evil.example/x"))
        assertFalse(AgentGit.openable("javascript:alert(1)"))
        assertTrue(AgentGit.openable("https://git.example.org/a/b/pull/1", host = "git.example.org"))
    }

    // ---- Messages ----

    @Test
    fun `a subject past 72 characters is long but never blocks`() {
        val long = "x".repeat(73)
        assertTrue(AgentGit.subjectLong(long + "\n\nbody"))
        assertFalse(AgentGit.subjectLong("x".repeat(72)))
        assertTrue(AgentGit.messageOk(long))
        assertFalse(AgentGit.messageOk("  \n "))
        assertFalse(AgentGit.messageOk("a\u0000b"))
        assertFalse(AgentGit.messageOk("é".repeat(4_097)))
        assertEquals("Fix it", AgentGit.subject("\n Fix it\n\nBecause."))
    }

    @Test
    fun `an agent's pull request answer splits into title and description`() {
        assertEquals("Keep rows" to "- waits for acks\n- backs off", AgentGit.prTitleBody("Keep rows\n\n- waits for acks\n- backs off\n"))
        assertEquals("Only a title" to "", AgentGit.prTitleBody("Only a title"))
        assertTrue(AgentGit.prOk("Keep rows", ""))
        assertFalse(AgentGit.prOk("", "body"))
        assertFalse(AgentGit.prOk("x".repeat(257), ""))
    }

    // ---- Branches ----

    private val branches = listOf(
        GitBranch("feat/sync-retry", current = true),
        GitBranch("origin/reports-weekly", remote = "origin"),
        GitBranch("main"),
        GitBranch("fix/receipt", worktree = "receipt"),
    )

    @Test
    fun `a folder with changes turns every branch off, the current one keeps its check`() {
        val choices = AgentGit.choices(branches, dirty = true, threadBusy = false, offline = false)
        assertEquals(listOf("feat/sync-retry", "main", "fix/receipt", "origin/reports-weekly"), choices.map { it.branch.name })
        assertEquals(listOf(null, BranchOff.DIRTY, BranchOff.DIRTY, BranchOff.DIRTY), choices.map { it.off })
        assertFalse(AgentGit.canCreate(dirty = true, threadBusy = false, offline = false))
        val clean = AgentGit.choices(branches, dirty = false, threadBusy = false, offline = false)
        assertEquals(listOf(null, null, BranchOff.OTHER_WORKTREE, null), clean.map { it.off })
        assertTrue(AgentGit.canCreate(dirty = false, threadBusy = false, offline = false))
        assertEquals(BranchOff.THREAD_BUSY, AgentGit.choices(branches, dirty = false, threadBusy = true, offline = false)[1].off)
        assertEquals(listOf("origin/reports-weekly"), AgentGit.search(branches, "WEEK").map { it.name })
    }

    @Test
    fun `a new branch name follows the PC's rules`() {
        for (bad in listOf("", "-x", "@", "HEAD", "a..b", "x.lock", "with space", "a//b", "a@{b", "end/", "end.", ".dot", "/lead", "x".repeat(101), "ünï")) {
            assertEquals(bad, NameProblem.BAD, AgentGit.nameProblem(bad, branches))
        }
        assertEquals(NameProblem.EXISTS, AgentGit.nameProblem("main", branches))
        assertNull(AgentGit.nameProblem("fix/ack-timeout", branches))
        assertNull("a remote's name isn't a local branch", AgentGit.nameProblem("origin/reports-weekly", branches))
    }

    // ---- Jobs ----

    private fun job(
        phase: JobPhase = JobPhase.RUNNING,
        lines: List<String> = emptyList(),
        from: Long = 0L,
        dropped: Int = 0,
        kind: JobKind = JobKind.COMMIT,
        error: JobError? = null,
        result: JobResult? = null,
    ) = GitJob(id = "j1", threadId = "t1", kind = kind, phase = phase, lines = lines, from = from, dropped = dropped, error = error, result = result)

    @Test
    fun `a job's lines arrive a few at a time and a finished job doesn't run again`() {
        var shown = AgentJobs.update(null, job(lines = listOf("Running your hooks", "cargo fmt --check")))
        shown = AgentJobs.update(shown, job(lines = listOf("cargo clippy"), from = 2))
        assertEquals(listOf("Running your hooks", "cargo fmt --check", "cargo clippy"), shown.lines)
        // Lines already kept aren't added twice.
        shown = AgentJobs.update(shown, job(lines = listOf("cargo clippy", "Checking duka-sync"), from = 2))
        assertEquals(listOf("Running your hooks", "cargo fmt --check", "cargo clippy", "Checking duka-sync"), shown.lines)
        shown = AgentJobs.update(shown, job(phase = JobPhase.DONE, from = 4, result = JobResult(oid7 = "9c41e7a", subject = "Keep sync rows")))
        shown = AgentJobs.update(shown, job(phase = JobPhase.RUNNING, lines = listOf("late"), from = 4))
        assertEquals(JobPhase.DONE, shown.phase)
        assertEquals(JobEnd.Committed("9c41e7a", "Keep sync rows"), AgentJobs.end(shown))
    }

    @Test
    fun `lines that went missing between updates make the phone read the job`() {
        val shown = AgentJobs.update(null, job(lines = listOf("a", "b")))
        val gap = job(lines = listOf("e"), from = 4)
        assertTrue(AgentJobs.needsRead(shown, gap))
        assertEquals(listOf("a", "b"), AgentJobs.update(shown, gap).lines)
        assertFalse(AgentJobs.needsRead(shown, job(lines = listOf("c"), from = 2)))
    }

    @Test
    fun `a push with nothing to send, a branch made and a pull request opened say so`() {
        assertEquals(JobEnd.UpToDate, AgentJobs.end(job(kind = JobKind.PUSH, phase = JobPhase.DONE, result = JobResult(upToDate = true))))
        assertEquals(JobEnd.Pushed("feat/x", "origin"), AgentJobs.end(job(kind = JobKind.PUSH, phase = JobPhase.DONE, result = JobResult(branch = "feat/x", remote = "origin"))))
        assertEquals(JobEnd.Switched("fix/y", created = true), AgentJobs.end(job(kind = JobKind.SWITCH, phase = JobPhase.DONE, result = JobResult(branch = "fix/y", created = true))))
        assertEquals(JobEnd.Opened(212), AgentJobs.end(job(kind = JobKind.PR_CREATE, phase = JobPhase.DONE, result = JobResult(number = 212))))
    }

    @Test
    fun `a phone keeps the last 200 lines and says how many weren't kept`() {
        var shown = AgentJobs.update(null, job(lines = List(150) { "a$it" }, dropped = 10))
        shown = AgentJobs.update(shown, job(lines = List(100) { "b$it" }, from = 150, dropped = 10))
        assertEquals(AgentJobs.MAX_LINES, shown.lines.size)
        assertEquals("b99", shown.lines.last())
        assertEquals(10 + 50, shown.notKept)
        assertEquals("numbers follow the lines kept", 50L, shown.from)
        // A read replaces what was kept.
        val read = AgentJobs.read(shown, job(lines = listOf("x"), dropped = 300))
        assertEquals(listOf("x"), read.lines)
        assertEquals(300, read.notKept)
    }

    @Test
    fun `a failed job's reason is worded for its kind`() {
        assertEquals(JobEnd.Failed(JobWhy.COMMIT_HOOK, "hook"), AgentJobs.end(job(phase = JobPhase.FAILED, error = JobError("hook"))))
        assertEquals(JobEnd.Failed(JobWhy.PUSH_HOOK, "hook"), AgentJobs.end(job(kind = JobKind.PUSH, phase = JobPhase.FAILED, error = JobError("hook"))))
        assertEquals(JobWhy.PUSH_BEHIND, AgentJobs.why(JobKind.PUSH, "behind"))
        assertEquals(JobWhy.FILES_CHANGED, AgentJobs.why(JobKind.COMMIT, "changed"))
        assertEquals(JobWhy.IDENTITY, AgentJobs.why(JobKind.COMMIT, "identity"))
        assertEquals(JobWhy.WOULD_OVERWRITE, AgentJobs.why(JobKind.SWITCH, "would_overwrite"))
        assertEquals(JobWhy.PR_EXISTS, AgentJobs.why(JobKind.PR_CREATE, "pr_exists"))
        assertEquals("an unknown word", JobWhy.PUSH_FAILED, AgentJobs.why(JobKind.PUSH, "something new"))
        assertEquals(JobEnd.Failed(JobWhy.COMMIT_STOPPED, "stopped"), AgentJobs.end(job(phase = JobPhase.STOPPED)))
        assertNull(AgentJobs.end(job()))
        assertTrue(AgentJobs.stoppable(job().copy(mine = true)))
        assertFalse("only one this phone started", AgentJobs.stoppable(job()))
        assertFalse(AgentJobs.stoppable(job(kind = JobKind.SWITCH).copy(mine = true)))
    }

    @Test
    fun `the jobs kept are the newest`() {
        var jobs = emptyMap<String, GitJob>()
        repeat(AgentJobs.MAX_JOBS + 3) { i -> jobs = AgentJobs.kept(jobs, job().copy(id = "j$i")) }
        assertEquals(AgentJobs.MAX_JOBS, jobs.size)
        assertFalse("j0" in jobs)
        assertTrue("j${AgentJobs.MAX_JOBS + 2}" in jobs)
    }

    // ---- "Ask <agent> to write it" ----

    private fun wrote(text: String, turn: Int = 3, purpose: WritePurpose = WritePurpose.COMMIT) = Wrote(purpose, turn, text, 1_000L * turn)

    @Test
    fun `the agent's answer fills a field nobody typed in since asking`() {
        val old = wrote("Old message", turn = 2)
        var field = AgentWrite.asked(WriteField(), old)
        assertTrue(field.asking)
        // The answer from before doesn't count.
        field = AgentWrite.thread(field, WritePurpose.COMMIT, old, running = true)
        assertTrue(field.asking)
        field = AgentWrite.thread(field, WritePurpose.COMMIT, wrote("Keep sync rows"), running = false)
        assertFalse(field.asking)
        assertEquals("Keep sync rows", field.text)
        assertNull(field.offer)
    }

    @Test
    fun `typed in meanwhile, the answer is offered instead`() {
        var field = AgentWrite.asked(WriteField(), null)
        field = AgentWrite.typed(field, "My own words")
        field = AgentWrite.thread(field, WritePurpose.COMMIT, wrote("Keep sync rows"), running = false)
        assertEquals("My own words", field.text)
        assertEquals("Keep sync rows", field.offer)
        assertEquals("Keep sync rows", AgentWrite.use(field, field.offer!!).text)
    }

    @Test
    fun `a pull request answer doesn't fill the commit message, and a turn ending without one stops asking`() {
        var field = AgentWrite.asked(WriteField(), null)
        field = AgentWrite.thread(field, WritePurpose.COMMIT, wrote("A title", purpose = WritePurpose.PR), running = true)
        assertTrue(field.asking)
        assertEquals("", field.text)
        field = AgentWrite.thread(field, WritePurpose.COMMIT, null, running = false)
        assertFalse(field.asking)
        assertFalse(AgentWrite.askFailed(AgentWrite.asked(WriteField(), null)).asking)
    }
}
