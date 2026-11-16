// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ble

/**
 * Runs GATT operations one at a time, in order: Android drops one started
 * while another is in flight. Each operation starts with [Op.start] (false if it
 * couldn't) and is finished by [complete] when its callback arrives. A failure
 * ends the queue and runs [onFailed] once; completions after that are ignored.
 */
class GattQueue(private val onFailed: (String) -> Unit) {
    fun interface Op {
        fun start(): Boolean
    }

    private class Named(val name: String, val op: Op)

    private val pending = ArrayDeque<Named>()
    private var inFlight: Named? = null
    private var failed = false

    @get:Synchronized
    val idle: Boolean
        get() = inFlight == null && pending.isEmpty()

    @Synchronized
    fun add(name: String, op: Op) {
        if (failed) return
        pending.addLast(Named(name, op))
        if (inFlight == null) next()
    }

    /** The operation in flight finished; [ok] false fails the queue. */
    @Synchronized
    fun complete(ok: Boolean) {
        val done = inFlight ?: return
        inFlight = null
        if (!ok) {
            fail("${done.name} failed")
            return
        }
        next()
    }

    @Synchronized
    fun fail(why: String) {
        if (failed) return
        failed = true
        pending.clear()
        inFlight = null
        onFailed(why)
    }

    private fun next() {
        while (!failed && inFlight == null) {
            val op = pending.removeFirstOrNull() ?: return
            inFlight = op
            val started = runCatching { op.op.start() }.getOrDefault(false)
            if (!started) {
                fail("${op.name} didn't start")
                return
            }
        }
    }
}
