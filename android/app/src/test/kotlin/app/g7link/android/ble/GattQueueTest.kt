// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ble

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class GattQueueTest {

    @Test
    fun `operations run one at a time and in order`() {
        val started = mutableListOf<String>()
        val queue = GattQueue { error("not expected") }
        queue.add("a") { started += "a"; true }
        queue.add("b") { started += "b"; true }
        queue.add("c") { started += "c"; true }
        assertEquals(listOf("a"), started)
        queue.complete(true)
        assertEquals(listOf("a", "b"), started)
        queue.complete(true)
        queue.complete(true)
        assertEquals(listOf("a", "b", "c"), started)
        assertTrue(queue.idle)
    }

    @Test
    fun `a failure ends the queue once and later completions are ignored`() {
        val failures = mutableListOf<String>()
        val started = mutableListOf<String>()
        val queue = GattQueue { failures += it }
        queue.add("mtu") { started += "mtu"; true }
        queue.add("services") { started += "services"; true }
        queue.complete(false)
        queue.complete(true)
        queue.add("write") { started += "write"; true }
        queue.fail("again")
        assertEquals(listOf("mtu"), started)
        assertEquals(listOf("mtu failed"), failures)
    }

    @Test
    fun `an operation that doesn't start fails the queue`() {
        val failures = mutableListOf<String>()
        val queue = GattQueue { failures += it }
        queue.add("write") { false }
        assertEquals(listOf("write didn't start"), failures)
        assertTrue(queue.idle)
    }
}
