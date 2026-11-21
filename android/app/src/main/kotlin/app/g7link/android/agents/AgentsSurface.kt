// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.agents

import app.g7link.android.ui.agents.AgentShelves
import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.ui.agents.Shelf
import app.g7link.android.ui.agents.ThreadStatus
import app.g7link.core.AgentThread

/** A thread's state as the tile and the widget show it. */
enum class SurfaceState { NEEDS_YOU, WORKING, DONE, PLAN, FAILED, PAUSED, STOPPED }

/**
 * One thread on the widget (contract §V8.3): its title, agent, folder and
 * state, and nothing else; never a request's command or an answer.
 */
data class SurfaceRow(
    val threadId: String,
    val title: String,
    val agent: String,
    val folder: String,
    val state: SurfaceState,
)

/** A PC's agents as the tile and the widget last heard them. */
data class SurfacePc(
    val pcId: String,
    val pcName: String,
    val online: Boolean,
    val needsYou: Int,
    val working: Int,
    /** Needs you first (oldest first), then working, then finished and unread; at most [AgentsSurfaces.MAX_ROWS]. */
    val rows: List<SurfaceRow>,
    val updatedMs: Long,
)

/** What the Quick Settings tile and the home-screen widget draw from, without the core running. */
data class AgentsSurface(val pcs: List<SurfacePc> = emptyList())

/** The tile's second line (counts only, on any screen). */
sealed interface TileLine {
    data class NeedsYou(val count: Int) : TileLine

    data class Working(val count: Int) : TileLine

    data object Nothing : TileLine

    data class Unreachable(val pcName: String) : TileLine
}

/** The tile: its line, lit only when something needs you, and what a tap opens (a PC, and its oldest waiting thread). */
data class TileView(val line: TileLine, val active: Boolean, val pcId: String?, val threadId: String?)

/** The widget: "Agents · <PC>", up to three rows, and whether that PC can't be reached now (its rows dimmed). */
data class WidgetView(val pcId: String?, val pcName: String?, val rows: List<SurfaceRow>, val offline: Boolean)

/**
 * The snapshot the Agents tile and widget read (contract §V8.3): counts and a
 * few rows per PC, written to app preferences by the core on agent events so
 * the surfaces draw without it. Pure: making it, reading it back, and what
 * each surface shows.
 */
object AgentsSurfaces {
    /** Rows kept per PC; the widget shows three. */
    const val MAX_ROWS = 4
    const val WIDGET_ROWS = 3

    /** PCs kept. */
    const val MAX_PCS = 4

    /** At most one write per this long; never on a timer of its own. */
    const val WRITE_EVERY_MS = 5_000L

    private const val MAX_TEXT = 200
    private const val VERSION = "agents-surface 1"

    /**
     * A PC's part of the snapshot from what the phone knows of its agents; null
     * when it offers none (nothing for the surfaces to show of it).
     */
    fun of(pcId: String, pcName: String, state: AgentsState, online: Boolean, nowMs: Long): SurfacePc? {
        if (state.catalog == null) return null
        val listed = state.threads.filter { AgentShelves.shelfOf(it) == Shelf.MAIN }
        val waiting = listed.filter(AgentsModel::needsYou).sortedWith(compareBy<AgentThread> { waitingSince(state, it) }.thenBy { it.id })
        val working = listed.filter { AgentsModel.running(it) && !AgentsModel.needsYou(it) }.sortedByDescending { it.updatedMs }
        val unread = listed.filter { !AgentsModel.running(it) && !AgentsModel.needsYou(it) && it.unread }.sortedByDescending { it.updatedMs }
        val rows = (waiting + working + unread).take(MAX_ROWS).map { thread ->
            SurfaceRow(
                threadId = thread.id,
                title = bound(thread.title),
                agent = bound(state.agentName(thread.agent)),
                folder = bound(thread.folder),
                state = stateOf(thread),
            )
        }
        return SurfacePc(pcId, bound(pcName), online, waiting.size, working.size, rows, nowMs)
    }

    private fun waitingSince(state: AgentsState, thread: AgentThread): Long {
        // Every request and question lasts as long: the one expiring first was asked first.
        val ask = state.asksFor(thread.id).firstOrNull()?.expiresMs
        val question = state.questionsFor(thread.id).firstOrNull()?.expiresMs
        return listOfNotNull(ask, question).minOrNull() ?: thread.updatedMs
    }

    fun stateOf(thread: AgentThread): SurfaceState = when (AgentsModel.status(thread)) {
        ThreadStatus.NeedsYou -> SurfaceState.NEEDS_YOU
        ThreadStatus.Working -> SurfaceState.WORKING
        ThreadStatus.Done -> SurfaceState.DONE
        ThreadStatus.PlanReady -> SurfaceState.PLAN
        ThreadStatus.Stopped -> SurfaceState.STOPPED
        is ThreadStatus.Paused -> SurfaceState.PAUSED
        is ThreadStatus.Failed -> SurfaceState.FAILED
    }

    /** [pc] put in place of what was kept for it, newest first, at most [MAX_PCS]. */
    fun with(surface: AgentsSurface, pc: SurfacePc): AgentsSurface =
        AgentsSurface((listOf(pc) + surface.pcs.filter { it.pcId != pc.pcId }).take(MAX_PCS))

    /** [pcId] can't be reached: its last rows stay, shown as such. */
    fun offline(surface: AgentsSurface, pcId: String): AgentsSurface =
        AgentsSurface(surface.pcs.map { if (it.pcId == pcId && it.online) it.copy(online = false) else it })

    /** Every PC can't be reached (G7 Link stopped). */
    fun allOffline(surface: AgentsSurface): AgentsSurface = AgentsSurface(surface.pcs.map { if (it.online) it.copy(online = false) else it })

    /** [pcId] was unpaired, or offers no agents now. */
    fun without(surface: AgentsSurface, pcId: String): AgentsSurface = AgentsSurface(surface.pcs.filter { it.pcId != pcId })

    /** Both show the same (when each was heard from aside): nothing to write. */
    fun sameShown(a: AgentsSurface, b: AgentsSurface): Boolean =
        a.pcs.map { it.copy(updatedMs = 0L) } == b.pcs.map { it.copy(updatedMs = 0L) }

    /**
     * The PC the surfaces show: a reachable one before one that isn't, then
     * the one where something needs you, then where something works, then the
     * one heard from last.
     */
    fun primary(surface: AgentsSurface): SurfacePc? = surface.pcs.maxWithOrNull(
        compareBy<SurfacePc> { it.online }.thenBy { it.needsYou > 0 }.thenBy { it.working > 0 }.thenBy { it.updatedMs },
    )

    /** "1 needs you" (lit) / "2 working" / "Nothing running" / "Kilele isn't reachable". */
    fun tile(surface: AgentsSurface): TileView {
        val pc = primary(surface) ?: return TileView(TileLine.Nothing, active = false, pcId = null, threadId = null)
        if (!pc.online) return TileView(TileLine.Unreachable(pc.pcName), active = false, pcId = pc.pcId, threadId = null)
        val oldest = pc.rows.firstOrNull { it.state == SurfaceState.NEEDS_YOU }?.threadId
        return when {
            pc.needsYou > 0 -> TileView(TileLine.NeedsYou(pc.needsYou), active = true, pcId = pc.pcId, threadId = oldest)
            pc.working > 0 -> TileView(TileLine.Working(pc.working), active = false, pcId = pc.pcId, threadId = null)
            else -> TileView(TileLine.Nothing, active = false, pcId = pc.pcId, threadId = null)
        }
    }

    fun widget(surface: AgentsSurface): WidgetView {
        val pc = primary(surface) ?: return WidgetView(null, null, emptyList(), offline = false)
        return WidgetView(pc.pcId, pc.pcName, pc.rows.take(WIDGET_ROWS), offline = !pc.online)
    }

    // ---- Kept in app preferences as lines of tab-separated, escaped fields ----

    fun encode(surface: AgentsSurface): String = buildString {
        append(VERSION).append('\n')
        for (pc in surface.pcs.take(MAX_PCS)) {
            line("P", pc.pcId, pc.pcName, if (pc.online) "1" else "0", pc.needsYou.toString(), pc.working.toString(), pc.updatedMs.toString())
            for (row in pc.rows.take(MAX_ROWS)) line("R", row.threadId, row.title, row.agent, row.folder, row.state.name)
        }
    }

    /** What [encode] wrote; anything it doesn't recognise is left out, and an unknown version is nothing. */
    fun decode(text: String?): AgentsSurface {
        val lines = text?.split('\n') ?: return AgentsSurface()
        if (lines.firstOrNull() != VERSION) return AgentsSurface()
        val pcs = ArrayList<SurfacePc>()
        for (raw in lines.drop(1)) {
            val fields = fields(raw)
            when (fields.firstOrNull()) {
                "P" -> {
                    if (pcs.size >= MAX_PCS || fields.size != 7) continue
                    val pcId = fields[1].takeIf { AgentsModel.isId(it, max = AgentsModel.MAX_PC_ID) } ?: continue
                    pcs += SurfacePc(
                        pcId = pcId,
                        pcName = bound(fields[2]),
                        online = fields[3] == "1",
                        needsYou = fields[4].toIntOrNull()?.coerceIn(0, MAX_COUNT) ?: 0,
                        working = fields[5].toIntOrNull()?.coerceIn(0, MAX_COUNT) ?: 0,
                        rows = emptyList(),
                        updatedMs = fields[6].toLongOrNull() ?: 0L,
                    )
                }
                "R" -> {
                    val pc = pcs.lastOrNull() ?: continue
                    if (pc.rows.size >= MAX_ROWS || fields.size != 6) continue
                    val threadId = fields[1].takeIf { AgentsModel.isId(it) } ?: continue
                    val state = SurfaceState.entries.firstOrNull { it.name == fields[5] } ?: continue
                    pcs[pcs.size - 1] = pc.copy(rows = pc.rows + SurfaceRow(threadId, bound(fields[2]), bound(fields[3]), bound(fields[4]), state))
                }
            }
        }
        return AgentsSurface(pcs)
    }

    private fun StringBuilder.line(vararg fields: String) {
        fields.joinTo(this, "\t") { escape(it) }
        append('\n')
    }

    private fun escape(value: String): String = buildString(value.length) {
        for (c in value) {
            when (c) {
                '\\' -> append("\\\\")
                '\t' -> append("\\t")
                '\n' -> append("\\n")
                '\r' -> append("\\r")
                else -> append(c)
            }
        }
    }

    private fun fields(line: String): List<String> {
        val out = ArrayList<String>()
        val current = StringBuilder()
        var i = 0
        while (i < line.length) {
            val c = line[i]
            when {
                c == '\t' -> {
                    out += current.toString()
                    current.setLength(0)
                }
                c == '\\' && i + 1 < line.length -> {
                    i++
                    current.append(
                        when (line[i]) {
                            't' -> '\t'
                            'n' -> '\n'
                            'r' -> '\r'
                            else -> line[i]
                        },
                    )
                }
                else -> current.append(c)
            }
            i++
        }
        out += current.toString()
        return out
    }

    /** One line, no control characters, at most [MAX_TEXT] characters (never half a surrogate pair). */
    private fun bound(value: String): String {
        val clean = value.map { if (it.isISOControl()) ' ' else it }.joinToString("").trim()
        if (clean.length <= MAX_TEXT) return clean
        val cut = clean.take(MAX_TEXT)
        return if (Character.isHighSurrogate(cut.last())) cut.dropLast(1) else cut
    }

    private const val MAX_COUNT = 999
}

/**
 * At most one write of the snapshot every [intervalMs] (contract §V8.3): a
 * change soon after the last write waits for the rest of the interval, and
 * changes meanwhile ride along. The clock is the caller's.
 */
class SurfaceThrottle(private val intervalMs: Long = AgentsSurfaces.WRITE_EVERY_MS) {
    private var lastMs: Long? = null
    private var waiting = false

    /** A change: 0 to write now, a delay to write after, or null when a write is already waiting. */
    @Synchronized
    fun changed(nowMs: Long): Long? {
        if (waiting) return null
        val last = lastMs
        if (last == null || nowMs - last !in 0 until intervalMs) return 0L
        waiting = true
        return intervalMs - (nowMs - last)
    }

    @Synchronized
    fun wrote(nowMs: Long) {
        lastMs = nowMs
        waiting = false
    }
}
