// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.agents

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.core.content.edit
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.widget.AgentsWidgetProvider

/**
 * The Agents tile's and widget's snapshot in app preferences (contract
 * §V8.3): written by the core as agent events change what they show, at most
 * once every 5 s, so they draw without it. They're asked to draw again after
 * each write; nothing here runs on a timer of its own.
 */
object AgentsSurfaceStore {
    private const val PREFS = "agents_surface"
    private const val KEY = "snapshot"

    private val throttle = SurfaceThrottle()
    private val main by lazy { Handler(Looper.getMainLooper()) }
    private val lock = Any()

    /** What was last decided, written or waiting to be. */
    private var latest: AgentsSurface? = null

    fun read(context: Context): AgentsSurface =
        runCatching { AgentsSurfaces.decode(prefs(context).getString(KEY, null)) }.getOrDefault(AgentsSurface())

    /** [pcId]'s threads now; a PC that offers no agents leaves the surfaces. */
    fun changed(context: Context, pcId: String, pcName: String, state: AgentsState, online: Boolean) {
        val part = AgentsSurfaces.of(pcId, pcName, state, online, System.currentTimeMillis())
        update(context) { surface -> if (part == null) AgentsSurfaces.without(surface, pcId) else AgentsSurfaces.with(surface, part) }
    }

    /** [pcId] can't be reached: its last rows stay, dimmed. */
    fun offline(context: Context, pcId: String) = update(context) { AgentsSurfaces.offline(it, pcId) }

    /** G7 Link stopped: every PC can't be reached from here. */
    fun allOffline(context: Context) = update(context) { AgentsSurfaces.allOffline(it) }

    /** [pcId] was unpaired. */
    fun forget(context: Context, pcId: String) = update(context) { AgentsSurfaces.without(it, pcId) }

    private fun update(context: Context, change: (AgentsSurface) -> AgentsSurface) {
        val app = context.applicationContext
        val wait = synchronized(lock) {
            val current = latest ?: read(app)
            val next = change(current)
            if (AgentsSurfaces.sameShown(current, next)) {
                latest = current
                return
            }
            latest = next
            throttle.changed(SystemClock.elapsedRealtime())
        }
        when (wait) {
            // A write already waits: this change goes with it.
            null -> Unit
            0L -> write(app)
            else -> main.postDelayed({ write(app) }, wait)
        }
    }

    private fun write(app: Context) {
        val snapshot = synchronized(lock) {
            throttle.wrote(SystemClock.elapsedRealtime())
            latest
        } ?: return
        runCatching { prefs(app).edit { putString(KEY, AgentsSurfaces.encode(snapshot)) } }
        AgentsTileService.refresh(app)
        AgentsWidgetProvider.refresh(app, snapshot)
    }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
