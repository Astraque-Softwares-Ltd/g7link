// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.companion

import android.content.Context
import android.util.Base64

/**
 * The beacon key of each paired PC, kept where the scan's receiver can read it
 * without starting the core. Only the beacon key, never the link secret: it lets
 * this phone recognise the PC's beacon and nothing more.
 */
object BeaconKeys {
    private const val PREFS = "ble_beacon"
    private const val KEY_LEN = 32

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Device ID to beacon key. */
    fun all(context: Context): Map<String, ByteArray> =
        prefs(context).all.mapNotNull { (id, value) ->
            val key = (value as? String)?.let { runCatching { Base64.decode(it, Base64.NO_WRAP) }.getOrNull() }
            key?.takeIf { it.size == KEY_LEN }?.let { id to it }
        }.toMap()

    /** Keeps exactly [keys]. Returns whether anything changed. */
    fun replace(context: Context, keys: Map<String, ByteArray>): Boolean {
        val wanted = keys.filterValues { it.size == KEY_LEN }
            .mapValues { (_, key) -> Base64.encodeToString(key, Base64.NO_WRAP) }
        val stored = prefs(context).all
        if (stored == wanted) return false
        prefs(context).edit().clear().apply {
            for ((id, key) in wanted) putString(id, key)
        }.apply()
        return true
    }
}
