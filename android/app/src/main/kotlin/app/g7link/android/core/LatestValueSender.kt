// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * Sends only the newest value per key. While a send for a key is in flight,
 * newer values [merge] into a single pending one, sent when that send ends:
 * at most one send per key runs and at most one waits, however fast values come.
 */
internal class LatestValueSender<K : Any, V : Any>(
    private val scope: CoroutineScope,
    private val merge: (pending: V, newer: V) -> V = { _, newer -> newer },
    private val onError: (Throwable) -> Unit = {},
    private val send: suspend (key: K, value: V) -> Unit,
) {
    private val lock = Any()
    private val pending = HashMap<K, V>()
    /** The running drain for each key that has one. */
    private val active = HashMap<K, Any>()

    fun offer(key: K, value: V) {
        val token = synchronized(lock) {
            pending[key] = pending[key]?.let { merge(it, value) } ?: value
            if (key in active) null else Any().also { active[key] = it }
        } ?: return
        scope.launch { drain(key) }.invokeOnCompletion {
            synchronized(lock) {
                if (active[key] === token) active.remove(key)
            }
        }
    }

    private suspend fun drain(key: K) {
        while (true) {
            val value = synchronized(lock) {
                pending.remove(key) ?: null.also { active.remove(key) }
            } ?: return
            try {
                send(key, value)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                onError(e)
            }
        }
    }
}

/**
 * Sums pointer deltas for one target until they are taken to be sent. A delta
 * for another target drops what was pending for the previous one.
 */
internal class DeltaCoalescer(private val limit: Float) {
    data class Delta(val target: String, val dx: Float, val dy: Float)

    private var target: String? = null
    private var dx = 0f
    private var dy = 0f
    private var queued = false

    /** Adds a delta; true when the caller must schedule a [take]. */
    @Synchronized
    fun add(id: String, dx: Float, dy: Float): Boolean {
        if (target != id) {
            this.dx = 0f
            this.dy = 0f
        }
        target = id
        this.dx += dx
        this.dy += dy
        if (queued) return false
        queued = true
        return true
    }

    /** The summed delta, within the protocol's [limit], or null when nothing moved. */
    @Synchronized
    fun take(): Delta? {
        val id = target
        val x = dx.coerceIn(-limit, limit)
        val y = dy.coerceIn(-limit, limit)
        dx = 0f
        dy = 0f
        queued = false
        return if (id != null && (x != 0f || y != 0f)) Delta(id, x, y) else null
    }

    /** Forgets what is pending, for when the scheduled [take] will never run. */
    @Synchronized
    fun clear() {
        dx = 0f
        dy = 0f
        queued = false
    }
}
