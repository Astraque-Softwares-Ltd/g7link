// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentHit
import app.g7link.core.AgentHits

/** Where a search stands. */
enum class SearchPhase {
    /** Nothing typed: no request. */
    IDLE,

    /** Typed; the request goes out once the typing pauses. */
    WAITING,
    SEARCHING,
    READY,
    FAILED,
}

/**
 * The search screen (contract §U9.1 "Search", reconcile §8): what's typed,
 * whether archived threads count, and the hits of the newest request. Every
 * change makes a new [generation]; an answer for an older one is dropped.
 */
@Immutable
data class SearchState(
    val query: String = "",
    val archived: Boolean = false,
    val generation: Long = 0L,
    val phase: SearchPhase = SearchPhase.IDLE,
    val hits: List<AgentHit> = emptyList(),
    /** The PC searched titles only (every word under 3 letters). */
    val titlesOnly: Boolean = false,
    /** Where the next page starts; null when there's none. */
    val next: String? = null,
    /** A next page is being read for this generation. */
    val loadingMore: Boolean = false,
    val failure: AgentFailure? = null,
)

/** A request to make for [generation]. */
@Immutable
data class SearchRequest(val generation: Long, val query: String, val archived: Boolean, val cursor: String? = null)

/** A thread's hits, in the PC's order: at most 3 shown, and how many more it has. */
@Immutable
data class HitGroup(
    val threadId: String,
    val title: String,
    val archived: Boolean,
    val whenMs: Long,
    /** The hits on messages; a title's own hit makes none. */
    val hits: List<AgentHit>,
    /** The title matched. */
    val titleHit: AgentHit?,
    val more: Int,
)

object AgentSearch {
    /** The request goes this long after the last key. */
    const val DEBOUNCE_MS = 150L

    /** Words shorter than this find titles only. */
    const val MIN_MESSAGE_CHARS = 3

    /** The longest query the PC takes, in bytes. */
    const val MAX_QUERY_BYTES = 200

    /** Hits a thread shows. */
    const val PER_THREAD = 3

    /** A page of hits. */
    const val PAGE = 30

    /** Every word is shorter than 3 letters: "Type 3 letters or more to search messages." */
    fun short(query: String): Boolean {
        val words = query.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }
        return words.isNotEmpty() && words.all { it.codePointCount(0, it.length) < MIN_MESSAGE_CHARS }
    }

    fun typed(state: SearchState, query: String): SearchState {
        if (query == state.query) return state
        val blank = query.isBlank()
        return state.copy(
            query = query,
            generation = state.generation + 1,
            phase = if (blank) SearchPhase.IDLE else SearchPhase.WAITING,
            hits = if (blank) emptyList() else state.hits,
            titlesOnly = if (blank) false else state.titlesOnly,
            next = null,
            loadingMore = false,
            failure = null,
        )
    }

    fun archivedChanged(state: SearchState, on: Boolean): SearchState {
        if (on == state.archived) return state
        return state.copy(
            archived = on,
            generation = state.generation + 1,
            phase = if (state.query.isBlank()) SearchPhase.IDLE else SearchPhase.WAITING,
            next = null,
            loadingMore = false,
            failure = null,
        )
    }

    /** The request to make once the typing paused; null when there's nothing to ask, or it's too long. */
    fun request(state: SearchState): SearchRequest? {
        if (state.phase != SearchPhase.WAITING) return null
        val query = state.query.trim()
        if (query.isEmpty() || AgentsModel.utf8Length(query) > MAX_QUERY_BYTES) return null
        return SearchRequest(state.generation, query, state.archived)
    }

    fun started(state: SearchState, generation: Long): SearchState =
        if (generation == state.generation && state.phase == SearchPhase.WAITING) state.copy(phase = SearchPhase.SEARCHING) else state

    /** The PC's answer for [generation]: replaces the hits, or is dropped when a newer one was asked since. */
    fun answered(state: SearchState, generation: Long, page: AgentHits): SearchState {
        if (generation != state.generation) return state
        return state.copy(phase = SearchPhase.READY, hits = page.hits, titlesOnly = page.titlesOnly, next = page.next, failure = null)
    }

    fun failed(state: SearchState, generation: Long, failure: AgentFailure): SearchState =
        if (generation != state.generation) state else state.copy(phase = SearchPhase.FAILED, failure = failure, loadingMore = false)

    /** The next page's request, when there is one and none is on its way. */
    fun more(state: SearchState): SearchRequest? {
        val cursor = state.next ?: return null
        if (state.phase != SearchPhase.READY || state.loadingMore) return null
        return SearchRequest(state.generation, state.query.trim(), state.archived, cursor)
    }

    fun loadingMore(state: SearchState, generation: Long): SearchState =
        if (generation == state.generation) state.copy(loadingMore = true) else state

    /** A next page: added after what's shown, without a hit twice. */
    fun moreAnswered(state: SearchState, generation: Long, page: AgentHits): SearchState {
        if (generation != state.generation) return state
        val seen = state.hits.mapTo(HashSet()) { it.thread to it.seq }
        return state.copy(
            hits = state.hits + page.hits.filter { (it.thread to it.seq) !in seen },
            next = page.next,
            loadingMore = false,
        )
    }

    /** Hits by thread, in the order the PC gave them (newest first). */
    fun groups(hits: List<AgentHit>): List<HitGroup> {
        val order = LinkedHashMap<String, MutableList<AgentHit>>()
        for (hit in hits) order.getOrPut(hit.thread) { ArrayList() }.add(hit)
        return order.map { (thread, all) ->
            val messages = all.filter { it.seq > 0uL }
            val shown = messages.take(PER_THREAD)
            val first = all.first()
            HitGroup(
                threadId = thread,
                title = first.title,
                archived = all.any { it.archived },
                whenMs = all.maxOf { it.whenMs },
                hits = shown,
                titleHit = all.firstOrNull { it.seq == 0uL },
                more = maxOf(all.maxOf { it.moreInThread.toInt() }, messages.size - shown.size),
            )
        }
    }

    /** A snippet's marks as ranges of its text, clamped to it. */
    fun markRanges(snippet: String, marks: List<app.g7link.core.AgentSnippetMark>): List<IntRange> =
        marks.mapNotNull { mark ->
            val start = mark.start.toLong().coerceIn(0L, snippet.length.toLong()).toInt()
            val end = (mark.start.toLong() + mark.len.toLong()).coerceIn(start.toLong(), snippet.length.toLong()).toInt()
            if (end > start) start until end else null
        }
}

/**
 * A thread opened at a message (a search hit): the screen scrolls to it and
 * lights it for a moment. One at a time; whoever opens the thread takes it.
 */
object AgentJump {
    /** How long the message stays lit. */
    const val HIGHLIGHT_MS = 1_200L

    @Immutable
    data class Target(val pcId: String, val threadId: String, val seq: ULong)

    @Volatile private var pending: Target? = null

    fun request(pcId: String, threadId: String, seq: ULong) {
        pending = if (seq > 0uL) Target(pcId, threadId, seq) else null
    }

    /** The message to open [threadId] at, once. */
    @Synchronized
    fun take(pcId: String, threadId: String): ULong? {
        val target = pending?.takeIf { it.pcId == pcId && it.threadId == threadId } ?: return null
        pending = null
        return target.seq
    }
}
