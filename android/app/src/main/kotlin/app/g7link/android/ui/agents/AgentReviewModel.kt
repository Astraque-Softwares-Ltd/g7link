// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable

/** How far a file has been read on the phone. */
enum class ReadMark { NOT_READ, READING, READ }

/** Why a file can't be read on the phone, so it's committed on the PC. */
enum class Unreadable {
    /** Its diff passes 1 MiB: "Too large to read here. Commit it on %1." */
    LARGE,

    /** An untracked folder of more than 50 entries. */
    FOLDER,
}

/** A changed file in the review: ticked to be committed, and how far it's been read. */
@Immutable
data class ReviewFile(
    val file: GitFile,
    val ticked: Boolean = true,
    val unreadable: Unreadable? = null,
    val mark: ReadMark = ReadMark.NOT_READ,
    /** A hash of the diff as it was read; the review the commit uses must have served the same bytes. */
    val readHash: String? = null,
    /** The review in use had the PC send this file's whole diff. */
    val served: Boolean = false,
    /** The hash of the diff the review in use served, until the file is read to its end. */
    val servedHash: String? = null,
) {
    val path: String get() = file.path

    val tickable: Boolean get() = unreadable == null
}

enum class ReviewStep { FILES, READ, MESSAGE }

/**
 * Review and commit on the phone (contract §SC6.3, §SC9.3): every changed file
 * is ticked; each is read whole, to its last line, before Commit can be
 * pressed; a change in the folder sends it back to the start. The PC checks
 * the same: it commits only a review whose every file it sent this phone in
 * full, and refuses one whose files changed since.
 */
@Immutable
data class ReviewState(
    val files: List<ReviewFile>,
    val branch: String? = null,
    /** The status these files came from: a different one means the folder changed. */
    val fingerprint: String? = null,
    val step: ReviewStep = ReviewStep.FILES,
    /** The file on screen while reading: an index into [files]. */
    val index: Int = 0,
    /** The review the PC keeps for these ticks, and the paths it covers. */
    val review: String? = null,
    val reviewPaths: List<String> = emptyList(),
    /** "The files changed. Read them again." */
    val changed: Boolean = false,
    /** Files past the phone's 200 a commit, left unticked. */
    val capped: Boolean = false,
) {
    val ticked: List<ReviewFile> get() = files.filter { it.ticked }

    val tickedPaths: List<String> get() = ticked.map { it.path }

    /** "Read %1 of %2". */
    val readCount: Int get() = ticked.count { it.mark == ReadMark.READ }

    val current: ReviewFile? get() = files.getOrNull(index)?.takeIf { step == ReviewStep.READ }
}

object AgentReview {
    /**
     * The review of [status]'s files: all ticked, up to the 200 a phone
     * commits. A folder change resets it with [changed] said once.
     */
    fun open(status: GitStatus, changed: Boolean = false): ReviewState {
        val files = status.files.mapIndexed { i, file -> ReviewFile(file, ticked = i < AgentGit.MAX_PHONE_PATHS) }
        return ReviewState(
            files = files,
            branch = status.branch,
            fingerprint = status.fingerprint,
            changed = changed,
            capped = status.files.size > AgentGit.MAX_PHONE_PATHS,
        )
    }

    /** The folder changed under the review (`agent.git`, or a new status): everything is read again. */
    fun folderChanged(state: ReviewState, status: GitStatus): ReviewState {
        if (!differs(state, status)) return state
        // Keeps what the user unticked, of files still there.
        val unticked = state.files.filterNot { it.ticked }.mapTo(HashSet()) { it.path }
        val fresh = open(status, changed = true)
        return fresh.copy(files = fresh.files.map { if (it.path in unticked) it.copy(ticked = false) else it })
    }

    /** Whether [status] isn't what the review was opened on. */
    fun differs(state: ReviewState, status: GitStatus): Boolean {
        val known = state.fingerprint
        if (known != null && status.fingerprint != null) return known != status.fingerprint
        return state.files.map { it.file } != status.files
    }

    /**
     * The PC's answer to a review of [paths]: an id, and which files it can't
     * send whole (unticked and off from now on). An answer for other ticks
     * than the ones now is dropped; a file found unreadable makes the review
     * cover the wrong files, so it's asked again.
     */
    fun reviewed(state: ReviewState, paths: List<String>, review: String, unreadable: Map<String, Unreadable>): ReviewState {
        if (paths != state.tickedPaths) return state
        val files = state.files.map { file ->
            val why = unreadable[file.path] ?: return@map file.copy(served = false, servedHash = null)
            file.copy(unreadable = why, ticked = false, mark = ReadMark.NOT_READ, readHash = null, served = false, servedHash = null)
        }
        val next = state.copy(files = files)
        return if (unreadable.keys.any { it in paths }) next.copy(review = null, reviewPaths = emptyList()) else next.copy(review = review, reviewPaths = paths)
    }

    /** A review has to be asked for: none yet, or the ticks changed since. */
    fun needsReview(state: ReviewState): Boolean = state.ticked.isNotEmpty() && (state.review == null || state.reviewPaths != state.tickedPaths)

    /** Ticks or unticks a file: an unreadable one stays off, and no more than 200 are ticked. */
    fun toggle(state: ReviewState, path: String): ReviewState {
        val at = state.files.indexOfFirst { it.path == path }
        val file = state.files.getOrNull(at) ?: return state
        if (!file.tickable) return state
        if (!file.ticked && state.ticked.size >= AgentGit.MAX_PHONE_PATHS) return state
        return state.copy(files = state.files.toMutableList().also { it[at] = file.copy(ticked = !file.ticked) }, changed = false)
    }

    /** "Start reading" / "Keep reading": the first ticked file not read yet (or the first ticked). */
    fun startReading(state: ReviewState): ReviewState {
        val ticked = state.files.withIndex().filter { it.value.ticked }
        if (ticked.isEmpty()) return state
        val at = (ticked.firstOrNull { it.value.mark != ReadMark.READ } ?: ticked.first()).index
        return reading(state.copy(step = ReviewStep.READ, index = at, changed = false))
    }

    /** Opens a ticked file from the list. */
    fun read(state: ReviewState, path: String): ReviewState {
        val at = state.files.indexOfFirst { it.path == path && it.ticked }
        if (at < 0) return state
        return reading(state.copy(step = ReviewStep.READ, index = at, changed = false))
    }

    private fun reading(state: ReviewState): ReviewState {
        val file = state.files.getOrNull(state.index) ?: return state
        if (file.mark != ReadMark.NOT_READ) return state
        return state.copy(files = state.files.toMutableList().also { it[state.index] = file.copy(mark = ReadMark.READING) })
    }

    /**
     * The file's diff arrived for the review in use: [whole] when the PC sent
     * all of it. A file already read keeps its mark only when the bytes are
     * the ones it was read as; otherwise it's read again. One the PC couldn't
     * send whole can't be read here: it's unticked and committed on the PC.
     */
    fun served(state: ReviewState, review: String, path: String, hash: String, whole: Boolean): ReviewState {
        if (review != state.review) return state
        val at = state.files.indexOfFirst { it.path == path }
        val file = state.files.getOrNull(at) ?: return state
        val next = when {
            !whole -> file.copy(unreadable = Unreadable.LARGE, ticked = false, served = false, servedHash = null, mark = ReadMark.NOT_READ, readHash = null)
            file.mark == ReadMark.READ && file.readHash != hash -> file.copy(served = true, servedHash = hash, mark = ReadMark.NOT_READ, readHash = null)
            else -> file.copy(served = true, servedHash = hash)
        }
        val files = state.files.toMutableList().also { it[at] = next }
        // A file that left the ticks leaves the review covering the wrong ones: it's asked for again.
        return if (!whole) state.copy(files = files, review = null, reviewPaths = emptyList(), step = ReviewStep.FILES) else state.copy(files = files)
    }

    /**
     * The file on screen was scrolled to its last line (or fitted the screen
     * for a second): read, as long as the review in use sent all of it.
     */
    fun reachedEnd(state: ReviewState, path: String): ReviewState {
        val at = state.files.indexOfFirst { it.path == path }
        val file = state.files.getOrNull(at) ?: return state
        if (!file.ticked || !file.served || file.servedHash == null) return state
        if (file.mark == ReadMark.READ && file.readHash == file.servedHash) return state
        return state.copy(files = state.files.toMutableList().also { it[at] = file.copy(mark = ReadMark.READ, readHash = file.servedHash) })
    }

    /** Files read before the review in use that it hasn't sent yet: asked for again, quietly, and checked against what was read. */
    fun toServe(state: ReviewState): List<String> =
        if (state.review == null) emptyList() else state.ticked.filter { it.mark == ReadMark.READ && !it.served }.map { it.path }

    /** "Next file" is pressed once the file on screen is read. */
    fun canNext(state: ReviewState): Boolean = state.current?.let { it.mark == ReadMark.READ && it.served } == true

    /** The next ticked file not read yet, after the one on screen, then from the start; the message once all are read. */
    fun next(state: ReviewState): ReviewState {
        if (!canNext(state)) return state
        val ticked = state.files.withIndex().filter { it.value.ticked }
        val after = ticked.firstOrNull { it.index > state.index && it.value.mark != ReadMark.READ }
            ?: ticked.firstOrNull { it.value.mark != ReadMark.READ }
        return if (after == null) state.copy(step = ReviewStep.MESSAGE) else reading(state.copy(index = after.index))
    }

    /** Back and forth among the ticked files is free. */
    fun previous(state: ReviewState): ReviewState {
        val before = state.files.withIndex().lastOrNull { it.index < state.index && it.value.ticked } ?: return state.copy(step = ReviewStep.FILES)
        return reading(state.copy(index = before.index))
    }

    /** Back to the list of files. */
    fun files(state: ReviewState): ReviewState = state.copy(step = ReviewStep.FILES)

    /** Every ticked file read whole under the review in use. */
    fun allRead(state: ReviewState): Boolean =
        state.ticked.isNotEmpty() && state.ticked.all { it.mark == ReadMark.READ && it.served && it.readHash == it.servedHash } &&
            state.review != null && state.reviewPaths == state.tickedPaths

    /** The message step opens once all are read. */
    fun toMessage(state: ReviewState): ReviewState = if (allRead(state)) state.copy(step = ReviewStep.MESSAGE) else state

    /** Commit can be pressed: every ticked file read, a review the PC keeps for exactly them, and a message it takes. */
    fun canCommit(state: ReviewState, message: String): Boolean =
        state.step == ReviewStep.MESSAGE && allRead(state) && AgentGit.messageOk(message)

    /** The review to commit with; null unless [canCommit]. */
    fun commitReview(state: ReviewState, message: String): String? = if (canCommit(state, message)) state.review else null

    /** A short hash of a diff's text, to tell later bytes from the ones read. */
    fun hash(patch: String): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256").digest(patch.toByteArray(Charsets.UTF_8))
        val hex = "0123456789abcdef"
        return buildString(32) {
            for (b in digest.take(16)) {
                val v = b.toInt() and 0xFF
                append(hex[v ushr 4])
                append(hex[v and 0x0F])
            }
        }
    }
}
