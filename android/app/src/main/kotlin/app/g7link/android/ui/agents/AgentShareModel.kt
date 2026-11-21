// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentThread

/**
 * What a share from another app brought to an agent's reply box (contract
 * §V6): the text, and the pictures copied into this app's cache until they're
 * uploaded or dropped. It's only ever put in the box; the user sends it.
 */
@Immutable
data class SharedDraft(
    val text: String,
    /** Cut to fit a message: the screen says "Shortened to fit." */
    val shortened: Boolean = false,
    /** Paths of the copies in `cacheDir/agent-share`. */
    val images: List<String> = emptyList(),
)

/** A picture offered by a share, as read: its name, and why it can't go (null when it can). */
@Immutable
data class SharedPicture(val name: String, val problem: ImageProblem?)

/** Why a shared item isn't added. */
enum class ShareRefusal {
    /** Not one of the four picture types G7 sends (a PDF, a video, a text file). */
    NOT_A_PICTURE,
    TOO_LARGE,

    /** Past the 4 pictures a message carries. */
    TOO_MANY,
}

/** Which of a share's pictures are added, by index, and which are refused, by name. */
@Immutable
data class SharePictures(val added: List<Int>, val refused: List<Pair<String, ShareRefusal>>)

/** The rules of sharing to an agent, apart from Android so they can be tested. */
object AgentShareRules {
    /** Threads offered to add to, newest first. */
    const val MAX_THREADS = 5

    /** What the share target takes with `ACTION_SEND`; `ACTION_SEND_MULTIPLE` takes `image/` only. */
    val SEND_TYPES = listOf("text/plain", "image/png", "image/jpeg", "image/webp", "image/gif")

    /**
     * The shared text as it goes in the box: the subject and the text joined
     * by a blank line, cut to what one message may be (a reply's 8,192 bytes
     * on the wire, and the box's 8,000 characters), never inside a character.
     * Null when nothing was shared as text.
     */
    fun text(subject: CharSequence?, text: CharSequence?): SharedDraft? {
        val joined = listOfNotNull(subject?.toString()?.trim()?.ifEmpty { null }, text?.toString()?.trim()?.ifEmpty { null })
            .joinToString("\n\n")
        if (joined.isEmpty()) return null
        var cut = AgentsModel.cutUtf8(joined, AgentsModel.MAX_PROMPT_BYTES)
        if (cut.length > AgentsModel.MAX_PROMPT_CHARS) {
            cut = cut.take(AgentsModel.MAX_PROMPT_CHARS)
            if (cut.isNotEmpty() && Character.isHighSurrogate(cut.last())) cut = cut.dropLast(1)
        }
        val shortened = cut.length < joined.length
        return SharedDraft(if (shortened) cut.trimEnd() else cut, shortened = shortened)
    }

    /** The first pictures that can go, up to the 4 a message carries; the rest refused by name. */
    fun pictures(read: List<SharedPicture>): SharePictures {
        val added = ArrayList<Int>()
        val refused = ArrayList<Pair<String, ShareRefusal>>()
        read.forEachIndexed { index, picture ->
            val refusal = when (picture.problem) {
                ImageProblem.NOT_AN_IMAGE, ImageProblem.FAILED -> ShareRefusal.NOT_A_PICTURE
                ImageProblem.TOO_LARGE -> ShareRefusal.TOO_LARGE
                null -> if (added.size >= AgentImages.MAX_PER_MESSAGE) ShareRefusal.TOO_MANY else null
            }
            if (refusal == null) added += index else refused += picture.name to refusal
        }
        return SharePictures(added, refused)
    }

    /** Whether a share's type is one the target takes; anything else never reaches the box. */
    fun takes(type: String?, multiple: Boolean): Boolean {
        val mime = type?.substringBefore(';')?.trim()?.lowercase() ?: return false
        return if (multiple) mime.startsWith("image/") else mime in SEND_TYPES
    }

    /** The threads a share can be added to: the 5 newest in the main list that aren't read-only. */
    fun threads(state: AgentsState): List<AgentThread> =
        state.listed.filter { it.readonly == null }.sortedByDescending { it.updatedMs }.take(MAX_THREADS)
}

/**
 * A share waiting for the screen it was sent to, once: a thread's reply box,
 * or a new thread on a PC. In memory: the share sheet hands it over and the
 * screen takes it as it opens.
 */
object AgentShared {
    @Immutable
    data class Target(val pcId: String, val threadId: String?)

    private var pending: Pair<Target, SharedDraft>? = null

    @Synchronized
    fun put(pcId: String, threadId: String?, draft: SharedDraft) {
        pending = Target(pcId, threadId) to draft
    }

    /** What was shared to [threadId] on [pcId] (null: a new thread), once. */
    @Synchronized
    fun take(pcId: String, threadId: String?): SharedDraft? {
        val (target, draft) = pending ?: return null
        if (target != Target(pcId, threadId)) return null
        pending = null
        return draft
    }

    /** The pictures of a share nobody took. */
    @Synchronized
    fun drop(): SharedDraft? {
        val draft = pending?.second
        pending = null
        return draft
    }
}

/** A thread to open with its reply box focused and nothing typed (the limit-reset notification's Open). */
object AgentFocus {
    @Volatile private var pending: Pair<String, String>? = null

    fun request(pcId: String, threadId: String) {
        pending = pcId to threadId
    }

    @Synchronized
    fun take(pcId: String, threadId: String): Boolean {
        if (pending != (pcId to threadId)) return false
        pending = null
        return true
    }
}
