// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

// A pull request read in full on the phone (phase 6 contract §X6.6): the card,
// who opened it and its description, the files it changes, its checks and its
// comments, read through the PC's GitHub CLI. Nothing here posts anything to
// GitHub. Plain Kotlin types, filled from the FFI in AgentGitFfi.kt only.

/** Why the view has no pull request to show, or shows an older read of it. */
enum class PrDetailWhy { NOT_GITHUB, NO_UPSTREAM, DETACHED, NETWORK, NOT_REPO, TOO_LARGE, NO_PR }

/** A review's word on a comment. */
enum class PrReviewWord { APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED }

/** A comment, or a review's summary, as the PC read it from GitHub. */
@Immutable
data class PrComment(
    val author: String,
    val atMs: Long,
    val body: String,
    /** Shortened to fit (4 KiB). */
    val cut: Boolean = false,
    val review: PrReviewWord? = null,
    val url: String? = null,
)

/** `agent.pr.detail`'s answer. */
@Immutable
data class PrDetail(
    /** The pull request as the card shows it; null when there's none to show ([why]). */
    val pr: PrInfo?,
    val gh: GhState,
    val why: PrDetailWhy? = null,
    /** `owner/name`, or `host/owner/name` off github.com. */
    val repo: String = "",
    val author: String = "",
    val body: String = "",
    /** The description was shortened to fit (32 KiB). */
    val bodyCut: Boolean = false,
    val adds: Int = 0,
    val dels: Int = 0,
    /** Files it changes in all. */
    val changed: Int = 0,
    val files: List<GitFile> = emptyList(),
    /** More files changed than are listed. */
    val filesMore: Boolean = false,
    /** Oldest first. */
    val comments: List<PrComment> = emptyList(),
    /** Older comments the PC didn't send. */
    val commentsOlder: Int = 0,
    /** A thread on this pull request's repository and branch. */
    val threadId: String? = null,
    val readMs: Long = 0L,
    /** The hosts the PC's GitHub CLI is signed in to. */
    val hosts: List<String> = emptyList(),
)

/** What the view shows. */
@Immutable
sealed interface PrDetailView {
    /** "Needs %1". */
    data object NeedsPc : PrDetailView

    data object Loading : PrDetailView

    data object Failed : PrDetailView

    /** No GitHub CLI on the PC, signed out, or too old. */
    data class Gh(val gh: GhState) : PrDetailView

    /** No pull request to show, and why. */
    data class Missing(val why: PrDetailWhy) : PrDetailView

    /** [unreached]: GitHub couldn't be reached, this is what was read then; [tooLarge]: its changes can't be listed. */
    data class Shown(val detail: PrDetail, val unreached: Boolean, val tooLarge: Boolean) : PrDetailView
}

/** "Ask <agent> to review it", or why it isn't offered. */
@Immutable
sealed interface PrAsk {
    /** "Needs %1". */
    data object NeedsPc : PrAsk

    /** "To ask an agent, open a thread in this repository, or ask on %1." */
    data object NoThread : PrAsk

    /** "Wait until %1 finishes.": the thread works or waits. */
    data object Busy : PrAsk

    /** Sent; the PC hasn't answered yet. */
    data object Asking : PrAsk

    /** The button, for this thread. */
    data class Ready(val threadId: String, val number: Int) : PrAsk
}

/** The rules of the pull request view. */
object AgentPrDetails {
    /** The view from the PC's answer: a pull request is always shown, with why it may be old; else the CLI, else why there's none. */
    fun view(call: AgentCall<PrDetail>?, offline: Boolean): PrDetailView = when (call) {
        null -> if (offline) PrDetailView.NeedsPc else PrDetailView.Loading
        is AgentCall.Failed -> if (call.failure == AgentFailure.OFFLINE) PrDetailView.NeedsPc else PrDetailView.Failed
        is AgentCall.Ok -> {
            val detail = call.value
            when {
                detail.pr != null -> PrDetailView.Shown(detail, unreached = detail.why == PrDetailWhy.NETWORK, tooLarge = detail.why == PrDetailWhy.TOO_LARGE)
                detail.gh != GhState.OK -> PrDetailView.Gh(detail.gh)
                else -> PrDetailView.Missing(detail.why ?: PrDetailWhy.NO_PR)
            }
        }
    }

    /**
     * The button: only with a thread (the one it was opened from, else one the
     * PC found on the pull request's repository and branch) that isn't
     * read-only; off while the PC can't be reached, the thread works or waits,
     * or an ask is on its way.
     */
    fun ask(detail: PrDetail, threadId: String?, readonly: Boolean, busy: Boolean, offline: Boolean, asking: Boolean): PrAsk {
        val pr = detail.pr
        val thread = threadId ?: detail.threadId
        if (pr == null || pr.number <= 0 || thread == null || readonly) return PrAsk.NoThread
        return when {
            offline -> PrAsk.NeedsPc
            asking -> PrAsk.Asking
            busy -> PrAsk.Busy
            else -> PrAsk.Ready(thread, pr.number)
        }
    }

    /** "%1 more files aren't listed.": past the 300 listed, when the PC says so. */
    fun moreFiles(detail: PrDetail): Int = if (detail.filesMore) (detail.changed - detail.files.size).coerceAtLeast(0) else 0

    /** "No comments yet.": none listed and none left out. */
    fun noComments(detail: PrDetail): Boolean = detail.comments.isEmpty() && detail.commentsOlder == 0

    /** A file's change as GitHub words it (lower-cased by the PC); a word this phone doesn't know reads as a change. */
    fun change(word: String): FileChange = when (word) {
        "added", "copied" -> FileChange.ADDED
        "deleted" -> FileChange.DELETED
        "renamed" -> FileChange.RENAMED
        else -> FileChange.MODIFIED
    }

    /** A review's word; any other is left out. */
    fun review(word: String?): PrReviewWord? = when (word) {
        "approved" -> PrReviewWord.APPROVED
        "changes_requested" -> PrReviewWord.CHANGES_REQUESTED
        "commented" -> PrReviewWord.COMMENTED
        "dismissed" -> PrReviewWord.DISMISSED
        else -> null
    }

    /** Comments as the view lists them: oldest first, ties in the PC's order. */
    fun ordered(comments: List<PrComment>): List<PrComment> = comments.sortedBy { it.atMs }

    /** The host a repository is on: the first part of `host/owner/name`, else github.com. */
    fun host(repo: String): String {
        val parts = repo.split('/')
        return if (parts.size == 3 && parts[0].isNotEmpty()) parts[0] else AgentGit.GITHUB_HOST
    }

    /** A file the diff can be asked for: one of the files the PC listed. */
    fun opens(detail: PrDetail, path: String): Boolean = detail.files.any { it.path == path }
}
