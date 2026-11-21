// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentBranch
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentCheckState
import app.g7link.core.AgentGhUse
import app.g7link.core.AgentGit
import app.g7link.core.AgentGitStrip
import app.g7link.core.AgentUnreadable
import app.g7link.core.AgentGitDoing
import app.g7link.core.AgentGitFileStatus
import app.g7link.core.AgentGitPatch
import app.g7link.core.AgentGitReview
import app.g7link.core.AgentJob
import app.g7link.core.AgentJobKind
import app.g7link.core.AgentJobPhase
import app.g7link.core.AgentPrBrief
import app.g7link.core.AgentPrInfo
import app.g7link.core.AgentPrReview
import app.g7link.core.AgentPrRows
import app.g7link.core.AgentPrShow
import app.g7link.core.AgentPrState
import app.g7link.core.AgentPrView
import app.g7link.core.AgentPrWhy
import app.g7link.core.AgentRepo
import app.g7link.core.AgentThread
import app.g7link.core.AgentWritePurpose
import app.g7link.core.Event
import app.g7link.core.G7LinkException

/**
 * The one place that reads the FFI's source-control records (contract
 * §SC6.4): the rest of Agents uses the plain types of AgentGitModel.kt.
 */
internal object AgentGitFfi {
    fun status(git: AgentGit): GitStatus = GitStatus(
        repo = when (git.repo) {
            AgentRepo.OK -> RepoKind.OK
            AgentRepo.NONE -> RepoKind.NONE
            AgentRepo.REFUSED -> RepoKind.REFUSED
            AgentRepo.NO_GIT -> RepoKind.NO_GIT
        },
        branch = git.branch,
        detached = git.detached,
        unborn = git.unborn,
        upstream = git.upstream,
        ahead = git.ahead?.toIntClamped(),
        behind = git.behind?.toIntClamped(),
        remote = git.remote,
        remotes = git.remotes.map { it.name },
        defaultBranch = git.defaultBranch,
        onDefault = git.onDefault,
        doing = git.doing?.let {
            when (it) {
                AgentGitDoing.MERGE -> GitDoing.MERGE
                AgentGitDoing.REBASE -> GitDoing.REBASE
                AgentGitDoing.CHERRY_PICK -> GitDoing.CHERRY_PICK
                AgentGitDoing.REVERT -> GitDoing.REVERT
                AgentGitDoing.BISECT -> GitDoing.BISECT
            }
        },
        locked = git.locked,
        files = git.files.map { file ->
            GitFile(
                path = file.path,
                from = file.from,
                change = when (file.status) {
                    // A word this phone doesn't know reads as a change.
                    null -> FileChange.MODIFIED
                    AgentGitFileStatus.ADDED -> FileChange.ADDED
                    AgentGitFileStatus.MODIFIED -> FileChange.MODIFIED
                    AgentGitFileStatus.DELETED -> FileChange.DELETED
                    AgentGitFileStatus.RENAMED -> FileChange.RENAMED
                    AgentGitFileStatus.TYPE -> FileChange.TYPE
                    AgentGitFileStatus.UNTRACKED -> FileChange.UNTRACKED
                    AgentGitFileStatus.CONFLICT -> FileChange.CONFLICT
                },
                folder = file.folder,
                adds = file.adds?.toIntClamped(),
                dels = file.dels?.toIntClamped(),
                binary = file.binary,
            )
        },
        more = git.more,
        total = git.total.toIntClamped(),
        adds = git.adds.toIntClamped(),
        dels = git.dels.toIntClamped(),
        conflicts = git.conflicts.toIntClamped(),
        last = git.last?.let { LastCommit(it.oid7, it.subject, it.ms) },
        worktree = git.worktree?.label,
        fetchedMs = git.fetchedMs,
        checkedMs = git.checkedMs,
        github = git.github?.repo,
        fingerprint = git.fingerprint.ifEmpty { null },
    )

    fun patch(patch: AgentGitPatch): GitPatch =
        GitPatch(patch.patch, patch.truncated, patch.binary, patch.names, patch.moreNames.toIntClamped())

    fun review(review: AgentGitReview): GitReviewAnswer = GitReviewAnswer(
        review = review.review,
        unreadable = review.files.filter { !it.readable }.associate { it.path to if (it.why == AgentUnreadable.FOLDER) Unreadable.FOLDER else Unreadable.LARGE },
    )

    fun branch(branch: AgentBranch): GitBranch = GitBranch(
        name = branch.name,
        remote = branch.remote,
        current = branch.current,
        upstream = branch.upstream,
        ahead = branch.ahead?.toIntClamped(),
        behind = branch.behind?.toIntClamped(),
        worktree = branch.worktree,
        lastMs = branch.lastMs,
    )

    fun job(job: AgentJob): GitJob = GitJob(
        id = job.id,
        threadId = job.thread,
        kind = when (job.kind) {
            AgentJobKind.COMMIT -> JobKind.COMMIT
            AgentJobKind.PUSH -> JobKind.PUSH
            AgentJobKind.FETCH -> JobKind.FETCH
            AgentJobKind.SWITCH -> JobKind.SWITCH
            AgentJobKind.WORKTREE -> JobKind.WORKTREE
            AgentJobKind.PR_CREATE -> JobKind.PR_CREATE
            AgentJobKind.PR_MERGE -> JobKind.PR_MERGE
        },
        phase = when (job.phase) {
            AgentJobPhase.RUNNING -> JobPhase.RUNNING
            AgentJobPhase.DONE -> JobPhase.DONE
            AgentJobPhase.FAILED -> JobPhase.FAILED
            AgentJobPhase.STOPPED -> JobPhase.STOPPED
        },
        startedMs = job.startedMs,
        endedMs = job.endedMs,
        mine = job.mine,
        lines = job.lines,
        from = job.from.toLongClamped(),
        dropped = job.dropped.toIntClamped(),
        result = job.result?.let { r ->
            JobResult(
                oid7 = r.oid7,
                subject = r.subject,
                files = r.files?.toIntClamped(),
                branch = r.branch,
                remote = r.remote,
                upToDate = r.upToDate,
                created = r.created,
                pr = r.pr?.let(::pr),
                number = r.number?.toIntClamped(),
            )
        },
        error = job.error?.let { JobError(it.reason, it.message, it.names) },
    )

    fun pr(pr: AgentPrInfo): PrInfo = PrInfo(
        number = pr.number.toIntClamped(),
        title = pr.title,
        state = prState(pr.state),
        draft = pr.draft,
        url = pr.url,
        base = pr.base,
        head = pr.head,
        review = pr.review?.let {
            when (it) {
                AgentPrReview.APPROVED -> PrReview.APPROVED
                AgentPrReview.CHANGES_REQUESTED -> PrReview.CHANGES_REQUESTED
                AgentPrReview.REVIEW_REQUIRED -> PrReview.REVIEW_REQUIRED
            }
        },
        conflicts = pr.mergeable == app.g7link.core.AgentMergeable.CONFLICTS,
        checks = pr.checks.map { PrCheck(it.name, checkState(it.state), it.url) },
        pass = pr.pass.toIntClamped(),
        fail = pr.fail.toIntClamped(),
        pending = pr.pending.toIntClamped(),
        readMs = pr.readMs,
    )

    fun brief(pr: AgentPrBrief): PrBrief =
        PrBrief(pr.number.toIntClamped(), prState(pr.state), pr.draft, pr.pass.toIntClamped(), pr.fail.toIntClamped(), pr.pending.toIntClamped())

    fun view(view: AgentPrView): PrView = PrView(
        pr = view.pr?.let(::pr),
        gh = gh(view.gh.gh),
        repo = view.github?.repo,
        why = when (view.why) {
            null -> null
            AgentPrWhy.NOT_GITHUB -> NoPr.NOT_GITHUB
            AgentPrWhy.NO_UPSTREAM -> NoPr.NO_UPSTREAM
            AgentPrWhy.DETACHED -> NoPr.DETACHED
            AgentPrWhy.NETWORK -> NoPr.NETWORK
            // Not a repository: no pull request can be opened from it, as from a remote not on GitHub.
            AgentPrWhy.NOT_REPO -> NoPr.NOT_GITHUB
        },
        host = view.github?.host,
        hosts = view.gh.hosts,
    )

    fun rows(rows: AgentPrRows): PrList = PrList(
        rows = rows.prs.map { row ->
            PrRow(
                number = row.number.toIntClamped(),
                title = row.title,
                repo = row.repo,
                state = prState(row.state),
                draft = row.draft,
                updatedMs = row.updatedMs,
                url = row.url,
                branch = row.branch,
                checks = row.checks?.let { PrChecks(it.pass.toIntClamped(), it.fail.toIntClamped(), it.pending.toIntClamped()) },
                threadId = row.thread,
            )
        },
        gh = gh(rows.gh.gh),
        more = rows.more,
        hosts = rows.gh.hosts,
    )

    fun show(show: PrShow): AgentPrShow = when (show) {
        PrShow.MINE -> AgentPrShow.MINE
        PrShow.REVIEW -> AgentPrShow.REVIEW
        PrShow.FOLDER -> AgentPrShow.FOLDER
    }

    fun purpose(purpose: WritePurpose): AgentWritePurpose = when (purpose) {
        WritePurpose.COMMIT -> AgentWritePurpose.COMMIT
        WritePurpose.PR -> AgentWritePurpose.PR
    }

    /** What the agent wrote when asked (`AgentThread.wrote`). */
    fun wrote(thread: AgentThread): Wrote? = thread.wrote?.let { wrote ->
        Wrote(
            purpose = when (wrote.purpose) {
                AgentWritePurpose.COMMIT -> WritePurpose.COMMIT
                AgentWritePurpose.PR -> WritePurpose.PR
            },
            turn = wrote.turn.toIntClamped(),
            text = wrote.text,
            atMs = wrote.atMs,
        )
    }

    /** This phone's "Push and open pull requests" switch on the PC (`AgentCatalog.push`). */
    fun pushAllowed(catalog: AgentCatalog?): Boolean = catalog?.push == true

    /** The strip's fields a thread carries: its branch, worktree and how many files changed. */
    fun stripOf(old: GitStrip?, thread: AgentThread): GitStrip? =
        AgentGitStrips.fromThread(old, thread.branch, thread.worktree?.label, thread.changed?.toIntClamped())

    /** An `agent.git` event as the strip takes it. */
    fun stripOf(old: GitStrip?, event: Event.AgentGit): GitStrip =
        AgentGitStrips.fromEvent(
            old,
            branch = event.branch,
            detached = event.detached,
            changed = event.changed.toIntClamped(),
            ahead = event.ahead?.toIntClamped(),
            behind = event.behind?.toIntClamped(),
            pr = event.pr?.let(::brief),
        )

    /** The strip this phone kept for reading while the PC is away. */
    fun strip(strip: AgentGitStrip): GitStrip = AgentGitStrips.fromEvent(
        null,
        branch = strip.branch,
        detached = strip.detached,
        changed = strip.changed.toIntClamped(),
        ahead = strip.ahead?.toIntClamped(),
        behind = strip.behind?.toIntClamped(),
        pr = strip.pr?.let(::brief),
    )

    /**
     * A refusal the PC answered with an error rather than a failed job: `BUSY: <word>` from the
     * host ("git_busy", "thread_busy"), and, for a commit, a review it no longer keeps (`NotFound`),
     * which reads as the files having changed.
     */
    fun refusal(e: G7LinkException, commit: Boolean = false): JobError? = when {
        e is G7LinkException.Internal && e.reason.startsWith(BUSY) -> JobError(e.reason.removePrefix(BUSY).trim().takeWhile { it.isLetterOrDigit() || it == '_' })
        e is G7LinkException.NotFound && commit -> JobError("changed")
        else -> null
    }

    private const val BUSY = "BUSY:"

    private fun prState(state: AgentPrState): PrState = when (state) {
        AgentPrState.OPEN -> PrState.OPEN
        AgentPrState.MERGED -> PrState.MERGED
        AgentPrState.CLOSED -> PrState.CLOSED
    }

    private fun checkState(state: AgentCheckState): CheckState = when (state) {
        AgentCheckState.PASS -> CheckState.PASS
        AgentCheckState.FAIL -> CheckState.FAIL
        AgentCheckState.PENDING -> CheckState.PENDING
        AgentCheckState.SKIPPED -> CheckState.SKIPPED
        AgentCheckState.CANCELLED -> CheckState.CANCELLED
    }

    private fun gh(gh: AgentGhUse): GhState = when (gh) {
        AgentGhUse.OK -> GhState.OK
        AgentGhUse.MISSING -> GhState.MISSING
        AgentGhUse.SIGNED_OUT -> GhState.SIGNED_OUT
        AgentGhUse.OLD -> GhState.OLD
    }

    private fun UInt.toIntClamped(): Int = if (this > Int.MAX_VALUE.toUInt()) Int.MAX_VALUE else toInt()

    private fun ULong.toLongClamped(): Long = if (this > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else toLong()
}
