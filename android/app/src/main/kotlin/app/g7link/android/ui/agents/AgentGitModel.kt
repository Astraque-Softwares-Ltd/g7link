// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

// The phone's part of Agents phase 4 ("Source control", contract §SC6, §SC9.1,
// §SC9.3): what a thread's folder looks like to git, its branches, a job's
// output, the branch's pull request. Plain Kotlin types, filled from the FFI
// in AgentGitFfi.kt only, so the rules here run in JVM tests.

/** What the folder is to git (`GitStatus.repo`). */
enum class RepoKind { OK, NONE, REFUSED, NO_GIT }

/** A changed file's state; its letter is the one the diff view already uses. */
enum class FileChange { ADDED, MODIFIED, DELETED, RENAMED, TYPE, UNTRACKED, CONFLICT }

/** Something git is in the middle of; a commit or a switch waits for it to finish in a terminal. */
enum class GitDoing { MERGE, REBASE, CHERRY_PICK, REVERT, BISECT }

/** A changed file, its path relative to the thread's folder. */
@Immutable
data class GitFile(
    val path: String,
    /** A rename's old path. */
    val from: String? = null,
    val change: FileChange,
    /** An untracked folder, collapsed to one row: ticking it commits what's inside. */
    val folder: Boolean = false,
    val adds: Int? = null,
    val dels: Int? = null,
    val binary: Boolean = false,
)

@Immutable
data class LastCommit(val oid7: String, val subject: String, val ms: Long)

/** The thread's folder as git sees it (`AgentGit`: at most 500 files reach a phone, never a URL or an absolute path). */
@Immutable
data class GitStatus(
    val repo: RepoKind,
    val branch: String? = null,
    /** The short commit HEAD is detached at. */
    val detached: String? = null,
    /** No commit yet. */
    val unborn: Boolean = false,
    val upstream: String? = null,
    /** Commits to push, against the upstream as last fetched. */
    val ahead: Int? = null,
    /** Commits on the remote this folder doesn't have, as of the last fetch. */
    val behind: Int? = null,
    /** The push remote, when git knows it. */
    val remote: String? = null,
    /** The remotes' names. */
    val remotes: List<String> = emptyList(),
    val defaultBranch: String? = null,
    val onDefault: Boolean = false,
    val doing: GitDoing? = null,
    /** `index.lock` exists: git is busy here. */
    val locked: Boolean = false,
    val files: List<GitFile> = emptyList(),
    /** More files changed than are listed. */
    val more: Boolean = false,
    val total: Int = 0,
    val adds: Int = 0,
    val dels: Int = 0,
    val conflicts: Int = 0,
    val last: LastCommit? = null,
    /** The worktree's label when the thread works in its own. */
    val worktree: String? = null,
    val fetchedMs: Long? = null,
    val checkedMs: Long = 0L,
    /** `owner/name` when the push remote is on GitHub. */
    val github: String? = null,
    /** Changes when anything shown here changes. */
    val fingerprint: String? = null,
) {
    /** Files that differ, as the strip counts them. */
    val changed: Int get() = maxOf(total, files.size)

    /** The remote a push goes to as the PC words it: the upstream's, else the push remote. */
    val pushRemote: String? get() = remote ?: upstream?.substringBefore('/')?.takeIf { it.isNotEmpty() && upstream.contains('/') }
}

enum class PrState { OPEN, MERGED, CLOSED }

/** A check's state; anything GitHub says that isn't understood is [PENDING], never [PASS]. */
enum class CheckState { PASS, FAIL, PENDING, SKIPPED, CANCELLED }

enum class PrReview { APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED }

@Immutable
data class PrCheck(val name: String, val state: CheckState, val url: String? = null)

/** The branch's pull request as GitHub last said (`PrInfo`). */
@Immutable
data class PrInfo(
    val number: Int,
    val title: String,
    val state: PrState,
    val draft: Boolean,
    val url: String,
    val base: String,
    val head: String,
    val review: PrReview? = null,
    /** GitHub says it conflicts with its base. */
    val conflicts: Boolean = false,
    val checks: List<PrCheck> = emptyList(),
    val pass: Int = 0,
    val fail: Int = 0,
    val pending: Int = 0,
    /** When the PC read it from GitHub. */
    val readMs: Long = 0L,
)

/** A pull request in brief, as the strip shows it. */
@Immutable
data class PrBrief(val number: Int, val state: PrState, val draft: Boolean, val pass: Int, val fail: Int, val pending: Int)

/** The GitHub CLI on the PC (`GhState.gh`). */
enum class GhState { OK, MISSING, SIGNED_OUT, OLD }

/** Why the branch shows no pull request. */
enum class NoPr { NOT_GITHUB, NO_UPSTREAM, DETACHED, NETWORK }

/** `agent/pr.view`'s answer: the pull request (the last one read when GitHub can't be reached), the CLI's state and why there's none. */
@Immutable
data class PrView(
    val pr: PrInfo?,
    val gh: GhState,
    val repo: String? = null,
    val why: NoPr? = null,
    /** The host the folder's repository is on. */
    val host: String? = null,
    /** The hosts the PC's GitHub CLI is signed in to. */
    val hosts: List<String> = emptyList(),
)

enum class PrShow { MINE, REVIEW, FOLDER }

@Immutable
data class PrChecks(val pass: Int, val fail: Int, val pending: Int)

/** A row of the Pull requests list. */
@Immutable
data class PrRow(
    val number: Int,
    val title: String,
    val repo: String,
    val state: PrState,
    val draft: Boolean,
    val updatedMs: Long,
    val url: String,
    val branch: String? = null,
    val checks: PrChecks? = null,
    val threadId: String? = null,
)

/** A branch the folder has, or one on a remote it hasn't checked out. */
@Immutable
data class GitBranch(
    val name: String,
    /** Set for a remote's branch ("On origin"). */
    val remote: String? = null,
    val current: Boolean = false,
    val upstream: String? = null,
    val ahead: Int? = null,
    val behind: Int? = null,
    /** Checked out in another worktree: its label. */
    val worktree: String? = null,
    val lastMs: Long = 0L,
)

enum class JobKind { COMMIT, PUSH, FETCH, SWITCH, WORKTREE, PR_CREATE, PR_MERGE }

enum class JobPhase { RUNNING, DONE, FAILED, STOPPED }

@Immutable
data class JobResult(
    val oid7: String? = null,
    val subject: String? = null,
    val files: Int? = null,
    val branch: String? = null,
    val remote: String? = null,
    /** A push that had nothing to send: "Already up to date". */
    val upToDate: Boolean = false,
    /** A switch that made the branch: "Created %1". */
    val created: Boolean = false,
    val pr: PrInfo? = null,
    /** The pull request a job opened. */
    val number: Int? = null,
)

/** A job that failed, or a refusal: the host's reason word (§SC3) and its message; [names] the remotes to pick from. */
@Immutable
data class JobError(val reason: String, val message: String = "", val names: List<String> = emptyList())

/** A commit, push, switch or pull request on the PC, and the lines its hooks and git wrote. */
@Immutable
data class GitJob(
    val id: String,
    val threadId: String,
    val kind: JobKind,
    val phase: JobPhase,
    val startedMs: Long = 0L,
    val endedMs: Long? = null,
    /** Started from this phone. */
    val mine: Boolean = false,
    val lines: List<String> = emptyList(),
    /** The number of `lines[0]` among every line the job printed, counting from 0. */
    val from: Long = 0L,
    /** Lines the PC didn't keep. */
    val dropped: Int = 0,
    /** Lines this phone didn't keep. */
    val cut: Int = 0,
    val result: JobResult? = null,
    val error: JobError? = null,
) {
    /** "%n earlier lines not kept". */
    val notKept: Int get() = dropped + cut
}

/** One file's uncommitted change as the PC sent it: all of it unless [truncated]; an untracked folder's names instead of a patch. */
@Immutable
data class GitPatch(
    val patch: String,
    val truncated: Boolean = false,
    val binary: Boolean = false,
    val names: List<String> = emptyList(),
    val moreNames: Int = 0,
) {
    /** What the review's check compares: the patch, or a folder's names, or the binary mark. */
    val content: String get() = when {
        binary -> "\u0000binary"
        names.isNotEmpty() || moreNames > 0 -> names.joinToString("\n") + "\n\u0000$moreNames"
        else -> patch
    }
}

/** The PC's review of the files a phone is about to read and commit: its token, and those it can't send whole. */
@Immutable
data class GitReviewAnswer(val review: String, val unreadable: Map<String, Unreadable>)

/** Starting a commit, push, switch or pull request: the job, or the PC's refusal and its reason word. */
@Immutable
sealed interface GitStart {
    data class Started(val job: GitJob) : GitStart

    data class Refused(val error: JobError?, val failure: AgentFailure) : GitStart
}

/** The Pull requests list's answer. */
@Immutable
data class PrList(val rows: List<PrRow>, val gh: GhState, val more: Boolean = false, val hosts: List<String> = emptyList())

/** What the strip under a thread's status line says (from the thread and the PC's `agent.git`). */
@Immutable
data class GitStrip(
    val branch: String? = null,
    val detached: String? = null,
    val worktree: String? = null,
    val changed: Int = 0,
    val ahead: Int? = null,
    val behind: Int? = null,
    val pr: PrBrief? = null,
)

enum class WritePurpose { COMMIT, PR }

/** What an agent wrote when asked for a commit message or a pull request (`AgentThread.wrote`). */
@Immutable
data class Wrote(val purpose: WritePurpose, val turn: Int, val text: String, val atMs: Long)

/** The panel's state line (§SC9.4 "States"), when one applies. */
@Immutable
sealed interface GitStateLine {
    data object NotRepo : GitStateLine

    data object NoGit : GitStateLine

    data object Refused : GitStateLine

    data object Busy : GitStateLine

    data class Doing(val doing: GitDoing) : GitStateLine

    data class Detached(val oid7: String) : GitStateLine

    data class Conflicts(val count: Int) : GitStateLine

    data object NoCommits : GitStateLine
}

/** The "Review and commit" row of the sheet. */
@Immutable
sealed interface ReviewRow {
    data object NeedsPc : ReviewRow

    data class Blocked(val line: GitStateLine) : ReviewRow

    /** "Wait until %1 finishes." */
    data object ThreadBusy : ReviewRow

    data object Nothing : ReviewRow

    data class Ready(val files: Int, val adds: Int, val dels: Int) : ReviewRow
}

/** The Push row: off with why, or what a push would send where. */
@Immutable
sealed interface PushRow {
    data object NeedsPc : PushRow

    data object Detached : PushRow

    data object NoCommits : PushRow

    data object NoRemote : PushRow

    data object UpToDate : PushRow

    /** "Turn on “Push and open pull requests” for this phone in G7 Link on %1." */
    data object SwitchOff : PushRow

    /**
     * A push of [branch] to [remote] (null: the PC asks which); [commits] when
     * the upstream is known, null for a branch not on a remote yet; [toDefault]:
     * the confirmation names the default branch.
     */
    data class Ready(val branch: String, val remote: String?, val commits: Int?, val toDefault: Boolean, val remotes: List<String> = emptyList()) : PushRow
}

/** The Pull request row. */
@Immutable
sealed interface PrRowState {
    data object NeedsPc : PrRowState

    data object Loading : PrRowState

    /** The branch's pull request; [unreached]: GitHub couldn't be reached, this is what was read then. */
    data class Shown(val pr: PrInfo, val unreached: Boolean) : PrRowState

    data class Gh(val gh: GhState) : PrRowState

    data object NotGithub : PrRowState

    data object NotPushed : PrRowState

    data object Detached : PrRowState

    data object Unreached : PrRowState

    data object SwitchOff : PrRowState

    data object CanOpen : PrRowState
}

/** The Branch row. */
@Immutable
sealed interface BranchRowState {
    data object NeedsPc : BranchRowState

    data object InWorktree : BranchRowState

    data class Ready(val branch: String?, val detached: String?) : BranchRowState
}

/** A checks line: "7 checks passed" · "2 of 7 failed" · "3 of 7 running" · "No checks". */
@Immutable
sealed interface ChecksLine {
    data object None : ChecksLine

    data class Passed(val count: Int) : ChecksLine

    data class Failed(val count: Int, val total: Int) : ChecksLine

    data class Running(val count: Int, val total: Int) : ChecksLine
}

/** One row of the branch sheet: [off] says why it can't be switched to. */
@Immutable
data class BranchChoice(val branch: GitBranch, val off: BranchOff? = null)

enum class BranchOff {
    /** "This folder has changes. Switch on %1." */
    DIRTY,

    /** "Open in another worktree (%1)" */
    OTHER_WORKTREE,

    /** "Wait until %1 finishes." */
    THREAD_BUSY,

    NEEDS_PC,
}

/** Why a typed branch name can't be made. */
enum class NameProblem { BAD, EXISTS }

/** The rules of the source-control strip, sheet and branch sheet (contract §SC9.1, §SC9.3). */
object AgentGit {
    /** A commit message's longest, in bytes, and the subject length past which it says "Subject is long". */
    const val MAX_MESSAGE_BYTES = 8_192
    const val SUBJECT_CHARS = 72

    /** A pull request's title and body bounds (bytes). */
    const val MAX_TITLE_BYTES = 256
    const val MAX_BODY_BYTES = 32_768

    /** A phone commits at most this many paths. */
    const val MAX_PHONE_PATHS = 200

    private const val MAX_BRANCH_BYTES = 100

    /** The strip shows for a repository: a branch, or a detached HEAD. */
    fun stripShown(strip: GitStrip?): Boolean = strip != null && (strip.branch != null || strip.detached != null)

    /** The strip from a full status read. */
    fun strip(status: GitStatus, pr: PrBrief?): GitStrip? =
        if (status.repo != RepoKind.OK) {
            null
        } else {
            GitStrip(
                branch = status.branch,
                detached = status.detached,
                worktree = status.worktree,
                changed = status.changed,
                ahead = status.ahead,
                behind = status.behind,
                pr = pr,
            )
        }

    /** The folder as the strip this phone kept says it, for the sheet while the PC is away. */
    fun offlineStatus(strip: GitStrip, savedMs: Long?): GitStatus = GitStatus(
        repo = RepoKind.OK,
        branch = strip.branch,
        detached = strip.detached,
        ahead = strip.ahead,
        behind = strip.behind,
        total = strip.changed,
        worktree = strip.worktree,
        checkedMs = savedMs ?: 0L,
    )

    /** The panel's state line, the most pressing first; null when nothing applies. */
    fun stateLine(status: GitStatus): GitStateLine? = when {
        status.repo == RepoKind.NONE -> GitStateLine.NotRepo
        status.repo == RepoKind.NO_GIT -> GitStateLine.NoGit
        status.repo == RepoKind.REFUSED -> GitStateLine.Refused
        status.locked -> GitStateLine.Busy
        status.doing != null -> GitStateLine.Doing(status.doing)
        status.conflicts > 0 -> GitStateLine.Conflicts(status.conflicts)
        status.detached != null -> GitStateLine.Detached(status.detached)
        status.unborn && status.files.isEmpty() -> GitStateLine.NoCommits
        else -> null
    }

    /** "In step with origin": an upstream, nothing to push or pull, nothing changed. */
    fun inStep(status: GitStatus): Boolean =
        status.repo == RepoKind.OK && status.upstream != null && (status.ahead ?: 0) == 0 && (status.behind ?: 0) == 0 && status.changed == 0

    /**
     * Review and commit: off while the PC can't be reached, while git or the
     * thread is busy, or with nothing to commit. A commit is refused on a
     * detached HEAD, mid-merge and with conflicts (§SC3.4), so those say why.
     */
    fun reviewRow(status: GitStatus, offline: Boolean, threadBusy: Boolean): ReviewRow {
        if (offline) return ReviewRow.NeedsPc
        stateLine(status)?.takeIf { it !is GitStateLine.NoCommits }?.let { return ReviewRow.Blocked(it) }
        if (status.changed == 0) return ReviewRow.Nothing
        if (threadBusy) return ReviewRow.ThreadBusy
        return ReviewRow.Ready(status.changed, status.adds, status.dels)
    }

    /**
     * Push: what can't be pushed says why, then the phone's switch on the PC
     * ("Push and open pull requests", off by default), then what goes where.
     */
    fun pushRow(status: GitStatus, offline: Boolean, pushAllowed: Boolean): PushRow {
        if (offline) return PushRow.NeedsPc
        if (status.repo != RepoKind.OK) return PushRow.NoRemote
        if (status.detached != null) return PushRow.Detached
        val branch = status.branch ?: return PushRow.Detached
        if (status.unborn) return PushRow.NoCommits
        if (status.remotes.isEmpty() && status.remote == null) return PushRow.NoRemote
        if (status.upstream != null && (status.ahead ?: 0) == 0) return PushRow.UpToDate
        if (!pushAllowed) return PushRow.SwitchOff
        val remote = pushRemoteOf(status)
        return PushRow.Ready(
            branch = branch,
            remote = remote,
            commits = if (status.upstream != null) status.ahead else null,
            toDefault = status.onDefault || (status.defaultBranch != null && status.defaultBranch == branch),
            remotes = if (remote == null) status.remotes.take(8) else emptyList(),
        )
    }

    /** The remote a push goes to: the upstream's, else `origin`, else the only one; null when the user picks. */
    fun pushRemoteOf(status: GitStatus): String? = status.pushRemote
        ?: status.remotes.firstOrNull { it == "origin" }
        ?: status.remotes.singleOrNull()

    /**
     * The Pull request row: a pull request the branch has is always shown
     * (reading needs no switch); otherwise what stops one, then the switch.
     */
    fun prRow(status: GitStatus, view: PrView?, offline: Boolean, pushAllowed: Boolean): PrRowState {
        if (offline) return PrRowState.NeedsPc
        if (view == null) return PrRowState.Loading
        view.pr?.let { return PrRowState.Shown(it, unreached = view.why == NoPr.NETWORK) }
        if (view.gh != GhState.OK) return PrRowState.Gh(view.gh)
        return when (view.why) {
            NoPr.NOT_GITHUB -> PrRowState.NotGithub
            NoPr.DETACHED -> PrRowState.Detached
            NoPr.NETWORK -> PrRowState.Unreached
            NoPr.NO_UPSTREAM -> PrRowState.NotPushed
            null -> when {
                status.detached != null -> PrRowState.Detached
                status.upstream == null || (status.ahead ?: 0) > 0 -> PrRowState.NotPushed
                !pushAllowed -> PrRowState.SwitchOff
                else -> PrRowState.CanOpen
            }
        }
    }

    fun branchRow(status: GitStatus, offline: Boolean): BranchRowState = when {
        offline -> BranchRowState.NeedsPc
        status.worktree != null -> BranchRowState.InWorktree
        else -> BranchRowState.Ready(status.branch, status.detached)
    }

    /** The checks line of a pull request: failing first, then running; skipped ones aren't counted. */
    fun checksLine(pr: PrInfo): ChecksLine {
        val total = if (pr.checks.isEmpty()) pr.pass + pr.fail + pr.pending else pr.checks.count { it.state != CheckState.SKIPPED }
        return checksLine(pr.pass, pr.fail, pr.pending, total)
    }

    fun checksLine(pass: Int, fail: Int, pending: Int, total: Int = pass + fail + pending): ChecksLine = when {
        fail > 0 -> ChecksLine.Failed(fail, maxOf(total, fail))
        pending > 0 -> ChecksLine.Running(pending, maxOf(total, pending))
        pass > 0 -> ChecksLine.Passed(pass)
        else -> ChecksLine.None
    }

    /** The strip's mark for a pull request's checks; null when it has none. */
    fun checksMark(pass: Int, fail: Int, pending: Int): CheckState? = when {
        fail > 0 -> CheckState.FAIL
        pending > 0 -> CheckState.PENDING
        pass > 0 -> CheckState.PASS
        else -> null
    }

    /** The checks as the card lists them: failing first, then running, the rest as GitHub gave them. */
    fun sortedChecks(checks: List<PrCheck>): List<PrCheck> = checks.sortedBy {
        when (it.state) {
            CheckState.FAIL -> 0
            CheckState.PENDING -> 1
            else -> 2
        }
    }

    fun brief(pr: PrInfo): PrBrief = PrBrief(pr.number, pr.state, pr.draft, pr.pass, pr.fail, pr.pending)

    /** The host a pull request is on when the PC doesn't say. */
    const val GITHUB_HOST = "github.com"

    /**
     * A link the phone may open: `https` only, and only on [host] when the PC
     * said which (the PC's core lets through only `https` links on a host its
     * GitHub CLI is signed in to).
     */
    fun openable(url: String, host: String? = GITHUB_HOST): Boolean {
        val parsed = runCatching { java.net.URI(url) }.getOrNull() ?: return false
        val at = parsed.host
        if (!parsed.scheme.equals("https", ignoreCase = true) || at.isNullOrEmpty()) return false
        return host == null || at.equals(host, ignoreCase = true)
    }

    /** A list row's link: `https` on a host the PC's GitHub CLI is signed in to (any `https` one when it named none). */
    fun openableOn(url: String, hosts: List<String>): Boolean =
        if (hosts.isEmpty()) openable(url, host = null) else hosts.any { openable(url, it) }

    // ---- The commit message (§SC3.1, §SC9.1 "Commit box") ----

    /** The first line, as the subject. */
    fun subject(message: String): String = message.trim().lineSequence().firstOrNull().orEmpty()

    /** Past 72 characters it says "Subject is long"; it never blocks. */
    fun subjectLong(message: String): Boolean = subject(message).codePointCount() > SUBJECT_CHARS

    /** A message the PC takes: 1 to 8,192 bytes after trimming, no NUL. */
    fun messageOk(message: String): Boolean {
        val trimmed = message.trim()
        return trimmed.isNotEmpty() && '\u0000' !in trimmed && AgentsModel.utf8Length(trimmed) <= MAX_MESSAGE_BYTES
    }

    /** A pull request's title and body that fit. */
    fun prOk(title: String, body: String): Boolean {
        val t = title.trim()
        return t.isNotEmpty() && '\u0000' !in t && '\u0000' !in body &&
            AgentsModel.utf8Length(t) <= MAX_TITLE_BYTES && AgentsModel.utf8Length(body) <= MAX_BODY_BYTES
    }

    /** An agent's answer for a pull request: the title on the first line, then the description after a blank line. */
    fun prTitleBody(text: String): Pair<String, String> {
        val lines = text.trim().lines()
        val title = lines.firstOrNull().orEmpty().trim()
        val body = lines.drop(1).joinToString("\n").trim()
        return title to body
    }

    // ---- Branches (§SC3.1 "A new branch name", §SC9.3 "Branch") ----

    /**
     * The branch sheet's rows: local ones first, then the remotes' (as the PC
     * lists them). The current one has the check and switches nowhere; with
     * changes in the folder, or while the thread works, every row is off.
     */
    fun choices(branches: List<GitBranch>, dirty: Boolean, threadBusy: Boolean, offline: Boolean): List<BranchChoice> =
        branches.sortedBy { if (it.remote == null) 0 else 1 }.map { branch ->
            val off = when {
                branch.current -> null
                offline -> BranchOff.NEEDS_PC
                dirty -> BranchOff.DIRTY
                threadBusy -> BranchOff.THREAD_BUSY
                branch.worktree != null -> BranchOff.OTHER_WORKTREE
                else -> null
            }
            BranchChoice(branch, off)
        }

    /** "New branch…" is off for the same reasons as switching. */
    fun canCreate(dirty: Boolean, threadBusy: Boolean, offline: Boolean): Boolean = !dirty && !threadBusy && !offline

    /** Branches whose name holds [query], case-folded. */
    fun search(branches: List<GitBranch>, query: String): List<GitBranch> {
        val q = query.trim().lowercase()
        if (q.isEmpty()) return branches
        return branches.filter { it.name.lowercase().contains(q) }
    }

    /**
     * Why a typed name can't be a new branch, with the PC's own rules: 1 to
     * 100 bytes of ASCII letters, digits and `/ . _ -`; not starting with `-`,
     * `/` or `.`; no `..`, `//` or `@{`; not ending in `/`, `.` or `.lock`;
     * not `HEAD` or `@`; and not a branch already. Null when it can be.
     */
    fun nameProblem(name: String, branches: List<GitBranch>): NameProblem? {
        val ok = name.isNotEmpty() && name.length <= MAX_BRANCH_BYTES &&
            name.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it in "/._-" } &&
            name.first() !in "-/." &&
            ".." !in name && "//" !in name && "@{" !in name &&
            !name.endsWith("/") && !name.endsWith(".") && !name.endsWith(".lock") &&
            name != "HEAD" && name != "@"
        if (!ok) return NameProblem.BAD
        if (branches.any { it.remote == null && it.name == name }) return NameProblem.EXISTS
        return null
    }

    private fun String.codePointCount(): Int = codePointCount(0, length)
}

/** A finished job's last line in words (§SC9.4 "Job lines", "Push", "Branches", "Open a PR"). */
@Immutable
sealed interface JobEnd {
    data class Committed(val oid7: String, val subject: String) : JobEnd

    data class Pushed(val branch: String, val remote: String) : JobEnd

    /** "Already up to date". */
    data object UpToDate : JobEnd

    data class Switched(val branch: String, val created: Boolean) : JobEnd

    data class Opened(val number: Int) : JobEnd

    data object Fetched : JobEnd

    /** Failed or stopped: [why] is what it says, [reason] the PC's word. */
    data class Failed(val why: JobWhy, val reason: String) : JobEnd
}

/** Why a job didn't go through, as the copy words it. */
enum class JobWhy {
    COMMIT_HOOK,
    COMMIT_FAILED,
    COMMIT_STOPPED,
    FILES_CHANGED,
    IDENTITY,
    NOTHING,
    THREAD_BUSY,
    SHARED_BUSY,
    GIT_BUSY,
    DOING,
    DETACHED,
    CONFLICTS,
    PUSH_BEHIND,
    PUSH_REFUSED,
    PUSH_HOOK,
    SIGN_IN,
    NETWORK,
    NO_REMOTE,
    PICK_REMOTE,
    PUSH_FAILED,
    NO_COMMITS,
    SWITCH_EXISTS,
    SWITCH_BAD_NAME,
    IN_WORKTREE,
    DIRTY,
    WOULD_OVERWRITE,
    SWITCH_FAILED,
    PR_EXISTS,
    NOT_PUSHED,
    NOT_GITHUB,
    NO_GH,
    SIGNED_OUT,
    OLD_GH,
    PR_FAILED,
}

/** Jobs as a phone follows them: lines arrive a few at a time, the last state wins. */
object AgentJobs {
    /** Lines a phone keeps of one job (the PC sends it the last 40 at most). */
    const val MAX_LINES = 200

    /** Lines shown before the rest scroll. */
    const val VISIBLE_LINES = 12

    /** Jobs kept per PC. */
    const val MAX_JOBS = 16

    /**
     * An `agent.git.progress` update: its lines are the new ones, numbered
     * from [GitJob.from]; they're added when they follow those kept, and
     * [needsRead] says when some went missing between (read the job then).
     * A job that ended doesn't go back to running.
     */
    fun update(old: GitJob?, update: GitJob): GitJob {
        if (old == null || old.id != update.id) return capped(update, 0)
        val ended = old.phase != JobPhase.RUNNING
        val phase = if (ended && update.phase == JobPhase.RUNNING) old.phase else update.phase
        val expected = old.from + old.lines.size
        // Lines already kept are skipped; a gap keeps what was there, and the job is read again.
        val skip = (expected - update.from).coerceIn(0L, update.lines.size.toLong()).toInt()
        val fresh = if (update.from > expected) emptyList() else update.lines.drop(skip)
        return capped(
            update.copy(
                phase = phase,
                lines = old.lines + fresh,
                from = old.from,
                dropped = maxOf(old.dropped, update.dropped),
                endedMs = update.endedMs ?: old.endedMs,
                result = update.result ?: old.result,
                error = update.error ?: old.error,
                startedMs = if (old.startedMs != 0L) old.startedMs else update.startedMs,
                mine = old.mine || update.mine,
            ),
            old.cut,
        )
    }

    /** Lines went missing between what's kept and [update]: the whole job is read. */
    fun needsRead(old: GitJob?, update: GitJob): Boolean =
        old != null && old.id == update.id && update.from > old.from + old.lines.size

    /** `agent/git.job`'s answer: the job as the PC keeps it, in place of what the phone had. */
    fun read(old: GitJob?, read: GitJob): GitJob {
        if (old != null && old.id == read.id && old.phase != JobPhase.RUNNING && read.phase == JobPhase.RUNNING) {
            return capped(read.copy(phase = old.phase, result = old.result, error = old.error, endedMs = old.endedMs), 0)
        }
        return capped(read, 0)
    }

    private fun capped(job: GitJob, cut: Int): GitJob {
        val over = job.lines.size - MAX_LINES
        return if (over <= 0) job.copy(cut = cut) else job.copy(lines = job.lines.drop(over), from = job.from + over, cut = cut + over)
    }

    /**
     * The jobs a PC's state keeps: this one in ([read]: a whole job the PC
     * answered with, else an update's new lines), the oldest out past [MAX_JOBS].
     */
    fun kept(jobs: Map<String, GitJob>, job: GitJob, read: Boolean = false): Map<String, GitJob> {
        val next = LinkedHashMap(jobs)
        next.remove(job.id)
        next[job.id] = if (read) read(jobs[job.id], job) else update(jobs[job.id], job)
        while (next.size > MAX_JOBS) next.remove(next.keys.first())
        return next
    }

    /** "Running your hooks…" shows while a commit or a push runs. */
    fun running(job: GitJob?): Boolean = job != null && job.phase == JobPhase.RUNNING

    /** Stop shows while a commit or a push this phone started runs. */
    fun stoppable(job: GitJob?): Boolean = running(job) && job?.mine == true && (job.kind == JobKind.COMMIT || job.kind == JobKind.PUSH)

    /** How a job ended, in words; null while it runs. */
    fun end(job: GitJob): JobEnd? {
        val result = job.result
        return when (job.phase) {
            JobPhase.RUNNING -> null
            JobPhase.DONE -> when (job.kind) {
                JobKind.COMMIT -> JobEnd.Committed(result?.oid7.orEmpty(), result?.subject.orEmpty())
                JobKind.PUSH -> if (result?.upToDate == true) JobEnd.UpToDate else JobEnd.Pushed(result?.branch.orEmpty(), result?.remote.orEmpty())
                JobKind.SWITCH -> JobEnd.Switched(result?.branch.orEmpty(), created = result?.created == true)
                JobKind.PR_CREATE -> JobEnd.Opened(result?.number ?: result?.pr?.number ?: 0)
                JobKind.FETCH, JobKind.WORKTREE, JobKind.PR_MERGE -> JobEnd.Fetched
            }
            JobPhase.STOPPED -> JobEnd.Failed(if (job.kind == JobKind.COMMIT) JobWhy.COMMIT_STOPPED else fallback(job.kind), "stopped")
            JobPhase.FAILED -> {
                val reason = job.error?.reason.orEmpty()
                JobEnd.Failed(why(job.kind, reason), reason)
            }
        }
    }

    /** A reason word (§SC3.4, §SC3.5, §SC3.7, §SC4.3) as the copy says it for a job of [kind]. */
    fun why(kind: JobKind, reason: String): JobWhy = when (reason) {
        "hook" -> when (kind) {
            JobKind.COMMIT -> JobWhy.COMMIT_HOOK
            JobKind.PUSH -> JobWhy.PUSH_HOOK
            else -> fallback(kind)
        }
        "changed" -> JobWhy.FILES_CHANGED
        "identity" -> JobWhy.IDENTITY
        "nothing" -> JobWhy.NOTHING
        "thread_busy" -> JobWhy.THREAD_BUSY
        "shared_busy" -> JobWhy.SHARED_BUSY
        "git_busy" -> JobWhy.GIT_BUSY
        "doing" -> JobWhy.DOING
        "detached" -> JobWhy.DETACHED
        "conflicts" -> JobWhy.CONFLICTS
        "behind" -> JobWhy.PUSH_BEHIND
        "remote_refused" -> JobWhy.PUSH_REFUSED
        "sign_in" -> JobWhy.SIGN_IN
        "network" -> JobWhy.NETWORK
        "no_remote" -> JobWhy.NO_REMOTE
        "pick_remote" -> JobWhy.PICK_REMOTE
        "no_commits" -> JobWhy.NO_COMMITS
        "exists" -> JobWhy.SWITCH_EXISTS
        "bad_name" -> JobWhy.SWITCH_BAD_NAME
        "in_worktree" -> JobWhy.IN_WORKTREE
        "dirty" -> JobWhy.DIRTY
        "would_overwrite" -> JobWhy.WOULD_OVERWRITE
        "pr_exists" -> JobWhy.PR_EXISTS
        "not_pushed" -> JobWhy.NOT_PUSHED
        "not_github" -> JobWhy.NOT_GITHUB
        "no_gh" -> JobWhy.NO_GH
        "signed_out" -> JobWhy.SIGNED_OUT
        "old_gh" -> JobWhy.OLD_GH
        else -> fallback(kind)
    }

    private fun fallback(kind: JobKind): JobWhy = when (kind) {
        JobKind.COMMIT -> JobWhy.COMMIT_FAILED
        JobKind.PUSH, JobKind.FETCH -> JobWhy.PUSH_FAILED
        JobKind.SWITCH, JobKind.WORKTREE -> JobWhy.SWITCH_FAILED
        JobKind.PR_CREATE, JobKind.PR_MERGE -> JobWhy.PR_FAILED
    }
}

/**
 * The message field and "Ask <agent> to write it" (contract §SC5.4): the
 * agent's answer fills the field when nothing was typed in it since asking;
 * otherwise it's offered ("Use %1's message").
 */
@Immutable
data class WriteField(
    val text: String = "",
    /** The answer in hand when asking, so a later one is told apart (null: not asking). */
    val askedOver: Wrote? = null,
    val asking: Boolean = false,
    /** The thread was seen working since the ask. */
    val sawWorking: Boolean = false,
    val typedSinceAsk: Boolean = false,
    /** An answer waiting while the field has the user's own words. */
    val offer: String? = null,
)

object AgentWrite {
    /** Asked: the answer before this one, if any, is remembered so it isn't taken for the new one. */
    fun asked(field: WriteField, current: Wrote?): WriteField =
        field.copy(askedOver = current, asking = true, sawWorking = false, typedSinceAsk = false, offer = null)

    /** The ask couldn't be sent. */
    fun askFailed(field: WriteField): WriteField = field.copy(asking = false, askedOver = null)

    fun typed(field: WriteField, text: String): WriteField =
        field.copy(text = text, typedSinceAsk = field.typedSinceAsk || field.asking)

    /** "Use the thread's title" or "Use %1's message". */
    fun use(field: WriteField, text: String): WriteField = field.copy(text = text, offer = null)

    /**
     * The thread changed while asking: a new answer for [purpose] fills the
     * field (or is offered when the user typed meanwhile); the turn ending
     * without one stops asking.
     */
    fun thread(field: WriteField, purpose: WritePurpose, wrote: Wrote?, running: Boolean): WriteField {
        if (!field.asking) return field
        if (wrote != null && wrote.purpose == purpose && wrote != field.askedOver && wrote.text.isNotBlank()) {
            return if (field.typedSinceAsk && field.text.isNotBlank()) {
                field.copy(asking = false, askedOver = null, offer = wrote.text)
            } else {
                field.copy(asking = false, askedOver = null, text = wrote.text, offer = null)
            }
        }
        if (running) return field.copy(sawWorking = true)
        return if (field.sawWorking) field.copy(asking = false, askedOver = null) else field
    }
}

/**
 * The strips a PC's threads show, from three sources: the thread itself
 * (its branch, worktree and count of changed files), the PC's `agent.git`
 * (ahead, behind, the pull request) and a status this phone read.
 */
object AgentGitStrips {
    /** Strips kept per PC. */
    const val MAX = 200

    /** A thread's own fields; null when it says nothing of git (not a repository, or never read). */
    fun fromThread(old: GitStrip?, branch: String?, worktree: String?, changed: Int?): GitStrip? {
        if (branch == null && changed == null && worktree == null) return old?.takeIf { it.detached != null }
        val base = old ?: GitStrip()
        return base.copy(
            branch = branch ?: base.branch,
            detached = if (branch != null) null else base.detached,
            worktree = worktree,
            changed = changed ?: base.changed,
        )
    }

    /** The PC's `agent.git`: what it read last, which replaces what was known. */
    fun fromEvent(old: GitStrip?, branch: String?, detached: Boolean, changed: Int, ahead: Int?, behind: Int?, pr: PrBrief?): GitStrip {
        val base = old ?: GitStrip()
        return base.copy(
            branch = branch,
            // The event names no commit; a detached one keeps the short id a status gave, else says "HEAD".
            detached = if (detached) base.detached ?: "HEAD" else null,
            changed = changed,
            ahead = ahead,
            behind = behind,
            pr = pr,
        )
    }

    /** A status read on this phone; the pull request stays as the PC last said. */
    fun fromStatus(old: GitStrip?, status: GitStatus): GitStrip? =
        AgentGit.strip(status, old?.pr)

    /** The strips with [threadId]'s one changed; a thread with none leaves the map. */
    fun put(strips: Map<String, GitStrip>, threadId: String, strip: GitStrip?): Map<String, GitStrip> {
        if (strips[threadId] == strip) return strips
        if (strip == null) return strips - threadId
        val next = LinkedHashMap(strips)
        next.remove(threadId)
        next[threadId] = strip
        while (next.size > MAX) next.remove(next.keys.first())
        return next
    }
}
