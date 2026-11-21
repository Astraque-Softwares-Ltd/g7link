// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.runtime.Immutable
import app.g7link.core.AgentRow
import app.g7link.core.AgentTimeline
import app.g7link.core.AgentTypingGuard
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A thread's rows and the input they were made from. */
@Immutable
data class TimelineRows(val input: TimelineInput, val rows: List<AgentRow>, val generation: Long)

/** When the rows are made again (contract §P6.1). */
object RowsSchedule {
    /** Text arriving makes the rows again at most this often. */
    const val DELTA_SPACING_MS = 100L

    /** Only the text still arriving changed: an item, the state, a request or a tap didn't. */
    fun onlyStreamingChanged(before: TimelineInput?, after: TimelineInput): Boolean =
        before != null && before.streaming != after.streaming && before.copy(streaming = after.streaming) == after

    /**
     * How long to wait before making the rows for [after]: text arriving is
     * gathered for up to [DELTA_SPACING_MS] after the last rows were made;
     * anything else is made at once.
     */
    fun waitMs(before: TimelineInput?, after: TimelineInput, lastAtMs: Long, nowMs: Long): Long =
        if (onlyStreamingChanged(before, after)) (lastAtMs + DELTA_SPACING_MS - nowMs).coerceIn(0L, DELTA_SPACING_MS) else 0L
}

/**
 * Which results may be shown: each input submitted gets the next generation, and
 * a result is shown only when it's newer than the one shown and made for the
 * thread on screen. A result for an input superseded before it was shown, or for
 * a thread left since, is dropped.
 */
class RowsGate(private val key: String) {
    private var submitted = 0L
    private var shown = 0L

    /** The generation of an input just submitted. */
    @Synchronized
    fun submit(): Long = ++submitted

    @Synchronized
    fun accept(result: TimelineRows): Boolean {
        if (result.input.key != key || result.generation <= shown || result.generation > submitted) return false
        shown = result.generation
        return true
    }
}

/**
 * Makes one thread's rows off the main thread (contract §P6.1): [submit] the
 * input whenever it may have changed; equal inputs are skipped, text arriving is
 * gathered to at most one recompute per [RowsSchedule.DELTA_SPACING_MS], the
 * newest input is the one made, and a stale result is dropped. [rows] is what
 * to draw. [make] is `AgentTimeline.rows`, which must not run on the main thread.
 */
class RowsFeed(
    key: String,
    private val make: (TimelineInput, Long) -> List<AgentRow>?,
    private val worker: CoroutineDispatcher = Dispatchers.Default,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private val gate = RowsGate(key)
    private val pending = MutableStateFlow<Pair<Long, TimelineInput>?>(null)
    private val _rows = MutableStateFlow<TimelineRows?>(null)
    val rows: StateFlow<TimelineRows?> = _rows.asStateFlow()

    fun submit(input: TimelineInput) {
        val current = pending.value
        if (current != null && current.second == input) return
        pending.value = gate.submit() to input
    }

    /** Runs until [scope] ends; one feed per thread screen. */
    fun start(scope: CoroutineScope): Job = scope.launch {
        var last: TimelineInput? = null
        var lastAt = 0L
        pending.filterNotNull().collect { first ->
            if (first.second == last) return@collect
            val wait = RowsSchedule.waitMs(last, first.second, lastAt, clock())
            if (wait > 0L) delay(wait)
            // The newest input while it waited: one recompute for every delta that came.
            val (generation, input) = pending.value ?: first
            if (input == last) return@collect
            val made = withContext(worker) { make(input, clock()) } ?: return@collect
            last = input
            lastAt = clock()
            val result = TimelineRows(input, made, generation)
            if (gate.accept(result)) _rows.value = result
        }
    }
}

/** [RowsFeed]'s maker over the FFI: one [AgentTimeline] per open thread, closed with it. */
class FfiRows : AutoCloseable {
    private val timeline = AgentTimeline()

    /** Null when the timeline was closed while this was on its way (the screen left). */
    fun make(input: TimelineInput, nowMs: Long): List<AgentRow>? = runCatching {
        // items, streaming, state, state_since_ms, now_ms, marks, open (contract §P4.3), turns and
        // their first files (§U9.1); a question's state comes from its item, so the waiting questions
        // aren't passed.
        timeline.rows(
            input.items,
            input.streaming,
            input.state,
            input.stateSinceMs,
            nowMs,
            input.marks,
            input.open,
            input.turns,
            input.turnFiles,
        )
    }.getOrNull()

    override fun close() = timeline.close()
}

/**
 * The typing guard (contract §P8.2): requests that arrive while you type are
 * held until you pause. The app uses the FFI's ([FfiTypingGuard]); JVM tests use
 * a Kotlin copy of the same rules.
 */
interface TypingGuardPort {
    /** The tokens of the thread's waiting requests, whenever that set changes. */
    fun requests(ids: List<String>, nowMs: Long)

    /** The reply box's text changed by the user's typing (not when it's set). */
    fun typed(nowMs: Long)

    fun focus()

    fun blur()

    /** A message was sent, or a request denied with words. */
    fun sent()

    /** Called at [idleAtMs]: the typing paused. */
    fun idle()

    fun held(id: String): Boolean

    /** When to call [idle] while anything is held; null when nothing is. */
    fun idleAtMs(): Long?
}

/** The FFI's guard. After [close] (the screen left) every call does nothing and nothing is held. */
class FfiTypingGuard : TypingGuardPort, AutoCloseable {
    private val guard = AgentTypingGuard()

    @Volatile private var closed = false

    private inline fun <T> use(otherwise: T, block: (AgentTypingGuard) -> T): T =
        if (closed) otherwise else runCatching { block(guard) }.getOrDefault(otherwise)

    override fun requests(ids: List<String>, nowMs: Long) = use(Unit) { it.requests(ids, nowMs) }

    override fun typed(nowMs: Long) = use(Unit) { it.typed(nowMs) }

    override fun focus() = use(Unit) { it.focus() }

    override fun blur() = use(Unit) { it.blur() }

    override fun sent() = use(Unit) { it.sent() }

    override fun idle() = use(Unit) { it.idle() }

    override fun held(id: String): Boolean = use(false) { it.held(id) }

    override fun idleAtMs(): Long? = use(null) { it.idleAtMs() }

    override fun close() {
        closed = true
        guard.close()
    }
}
