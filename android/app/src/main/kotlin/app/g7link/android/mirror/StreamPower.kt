// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.mirror

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.PowerManager
import app.g7link.android.elevated.Elevated

/**
 * The phone's power while it streams to a PC: its screen ([SCREEN]) or apps
 * in windows of their own ([APPS]). Each says what it wants with [set], and
 * the phone does the most any of them asks for, so neither undoes the other.
 *
 * - While anything streams, a CPU wake lock and a Wi-Fi lock keep the phone
 *   on the network with its screen off. Without them it dozes, misses the
 *   PC's keep-alives, and the PC drops the connection after 8 s; the stream
 *   then restarts every few seconds.
 * - Stay awake keeps the screen on; screen off keeps it on too, with the
 *   panel turned off through Elevated, so apps on the PC keep running.
 * - While the phone shows a PC's screen ([VIEW]: the Screen tab, Play on
 *   phone), a low-latency Wi-Fi lock keeps its radio from dozing between
 *   packets, which otherwise holds the picture back.
 */
object StreamPower {
    const val SCREEN = "screen"
    const val APPS = "apps"

    /** The phone shows a PC's screen: the Wi-Fi lock only, as its own screen is on. */
    const val VIEW = "view"

    private data class Want(val stayAwake: Boolean, val screenOff: Boolean)

    private val wants = HashMap<String, Want>()
    private var cpu: PowerManager.WakeLock? = null
    private var wifi: WifiManager.WifiLock? = null
    private var lowLatency: WifiManager.WifiLock? = null
    private var screen: PowerManager.WakeLock? = null
    /** What Elevated was last told; the panel is on until something turns it off. */
    private var panelOff = false

    /** [source] streams ([streaming]), and wants the phone kept awake or its screen off. */
    @Synchronized
    fun set(context: Context, source: String, streaming: Boolean, stayAwake: Boolean = false, screenOff: Boolean = false) {
        if (streaming) wants[source] = Want(stayAwake, screenOff) else wants.remove(source)
        apply(context.applicationContext)
    }

    @Suppress("DEPRECATION")
    private fun apply(context: Context) {
        val streaming = wants.keys.any { it != VIEW }
        val viewing = VIEW in wants
        val off = wants.values.any { it.screenOff }
        val awake = off || wants.values.any { it.stayAwake }
        val power = context.getSystemService(PowerManager::class.java)
        val wifiManager = context.getSystemService(WifiManager::class.java)

        cpu = hold(cpu, streaming) { power?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "g7link:stream") }
        // HIGH_PERF: Wi-Fi out of power save with the screen off (newer
        // Android treats it as low latency, which only helps with it on).
        wifi = if (streaming) {
            (wifi ?: wifiManager?.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "g7link:stream"))
                ?.apply {
                    setReferenceCounted(false)
                    if (!isHeld) runCatching { acquire() }
                }
        } else {
            wifi?.let { if (it.isHeld) runCatching { it.release() } }
            null
        }
        // LOW_LATENCY (Android 10 and later) works while the screen is on and the
        // app in front, which is when a PC's screen shows.
        lowLatency = if (viewing) {
            val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
                WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            (lowLatency ?: wifiManager?.createWifiLock(mode, "g7link:view"))
                ?.apply {
                    setReferenceCounted(false)
                    if (!isHeld) runCatching { acquire() }
                }
        } else {
            lowLatency?.let { if (it.isHeld) runCatching { it.release() } }
            null
        }
        screen = hold(screen, awake) {
            power?.newWakeLock(
                PowerManager.SCREEN_BRIGHT_WAKE_LOCK or PowerManager.ACQUIRE_CAUSES_WAKEUP,
                "g7link:mirror",
            )
        }
        if (panelOff != off) {
            panelOff = off
            Elevated.setScreenOff(off)
        }
    }

    /** Holds [lock] (made with [make] the first time) while [on], releases it otherwise. */
    private fun hold(lock: PowerManager.WakeLock?, on: Boolean, make: () -> PowerManager.WakeLock?): PowerManager.WakeLock? {
        if (!on) {
            lock?.let { if (it.isHeld) runCatching { it.release() } }
            return null
        }
        val held = (lock ?: make()?.apply { setReferenceCounted(false) }) ?: return null
        // Renewed on every change; a stream left open longer lets go by itself.
        runCatching { held.acquire(HOLD_MAX_MS) }
        return held
    }

    private const val HOLD_MAX_MS = 4 * 60 * 60 * 1000L
}
