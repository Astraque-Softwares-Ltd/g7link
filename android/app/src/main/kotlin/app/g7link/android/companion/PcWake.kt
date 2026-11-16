// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import android.content.Context
import android.os.SystemClock
import app.g7link.android.G7LinkApplication
import app.g7link.android.core.CoreStatus
import app.g7link.android.service.ConnectionService
import app.g7link.android.ui.Preferences

/**
 * A paired PC is nearby: the connection service starts and reconnects now,
 * at most once per PC every [MIN_GAP_MS] (a beacon can be seen many times a
 * second, and a recorded one replayed).
 */
object PcWake {
    private const val MIN_GAP_MS = 30_000L
    private val lastWake = HashMap<String, Long>()

    /** Whether a wake for [pcId] at [nowMs] (elapsed realtime) goes ahead. */
    @Synchronized
    internal fun allowed(pcId: String, nowMs: Long, last: MutableMap<String, Long> = lastWake): Boolean {
        val previous = last[pcId]
        if (previous != null && nowMs - previous in 0L until MIN_GAP_MS) return false
        last[pcId] = nowMs
        return true
    }

    /** Wakes the app for [pcId], or for any PC when it's null. */
    fun wake(context: Context, pcId: String?) {
        val appContext = context.applicationContext ?: return
        if (!Preferences.isBackgroundServiceEnabled(appContext)) return
        if (!allowed(pcId ?: "", SystemClock.elapsedRealtime())) return
        val core = (appContext as? G7LinkApplication)?.core
        val state = core?.state?.value
        if (state != null && state.status is CoreStatus.Ready && state.devices.isEmpty()) {
            PcCompanion.disassociateForRemovedPc(appContext, pcId = null, pcName = null, noDevicesRemaining = true)
            return
        }
        if (pcId != null && state != null && state.devices.any { it.id == pcId && it.online }) return
        ConnectionService.start(appContext)
        core?.reconnectNow()
    }
}
