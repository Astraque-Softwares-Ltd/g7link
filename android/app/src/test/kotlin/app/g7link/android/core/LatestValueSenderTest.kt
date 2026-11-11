// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LatestValueSenderTest {
    @Test
    fun valuesOfferedDuringASendCollapseIntoTheLatest() = runBlocking {
        val sent = mutableListOf<Int>()
        val firstStarted = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val lastSent = CompletableDeferred<Unit>()
        val sender = LatestValueSender<String, Int>(scope = this) { _, value ->
            sent += value
            if (!firstStarted.isCompleted) {
                firstStarted.complete(Unit)
                release.await()
            }
            if (value == 100) lastSent.complete(Unit)
        }

        sender.offer("pc", 1)
        firstStarted.await()
        for (value in 2..100) sender.offer("pc", value)
        release.complete(Unit)
        withTimeout(5_000) { lastSent.await() }

        assertEquals(listOf(1, 100), sent)
    }

    @Test
    fun pendingValuesMerge() = runBlocking {
        val sent = mutableListOf<Pair<Int?, Boolean?>>()
        val firstStarted = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val sender = LatestValueSender<String, Pair<Int?, Boolean?>>(
            scope = this,
            merge = { pending, newer -> (newer.first ?: pending.first) to (newer.second ?: pending.second) },
        ) { _, value ->
            sent += value
            if (!firstStarted.isCompleted) {
                firstStarted.complete(Unit)
                release.await()
            }
        }

        sender.offer("pc", 10 to null)
        firstStarted.await()
        sender.offer("pc", null to true)
        sender.offer("pc", 40 to null)
        release.complete(Unit)
        withTimeout(5_000) { while (sent.size < 2) kotlinx.coroutines.yield() }

        assertEquals(listOf<Pair<Int?, Boolean?>>(10 to null, 40 to true), sent)
    }

    @Test
    fun keysDoNotWaitForEachOther() = runBlocking {
        val sent = mutableListOf<String>()
        val blocked = CompletableDeferred<Unit>()
        val sender = LatestValueSender<String, Int>(scope = this) { key, _ ->
            if (key == "slow") blocked.await()
            sent += key
        }

        sender.offer("slow", 1)
        sender.offer("fast", 1)
        withTimeout(5_000) { while ("fast" !in sent) kotlinx.coroutines.yield() }
        val whileSlowWaits = sent.toList()
        blocked.complete(Unit)
        withTimeout(5_000) { while ("slow" !in sent) kotlinx.coroutines.yield() }

        assertEquals(listOf("fast"), whileSlowWaits)
    }

    @Test
    fun aFailedSendDoesNotStopLaterOnes() = runBlocking {
        val errors = mutableListOf<Throwable>()
        val sent = mutableListOf<Int>()
        val sender = LatestValueSender<String, Int>(scope = this, onError = { errors += it }) { _, value ->
            if (value == 1) error("no reply")
            sent += value
        }

        sender.offer("pc", 1)
        withTimeout(5_000) { while (errors.isEmpty()) kotlinx.coroutines.yield() }
        sender.offer("pc", 2)
        withTimeout(5_000) { while (sent.isEmpty()) kotlinx.coroutines.yield() }

        assertEquals(1, errors.size)
        assertEquals(listOf(2), sent)
    }

    @Test
    fun aValueAfterTheLastSendStartsANewOne() = runBlocking {
        val sent = mutableListOf<Int>()
        val sender = LatestValueSender<String, Int>(scope = this) { _, value -> sent += value }

        sender.offer("pc", 1)
        withTimeout(5_000) { while (sent.size < 1) kotlinx.coroutines.yield() }
        sender.offer("pc", 2)
        withTimeout(5_000) { while (sent.size < 2) kotlinx.coroutines.yield() }

        assertEquals(listOf(1, 2), sent)
    }
}

class DeltaCoalescerTest {
    @Test
    fun deltasBeforeATakeAreSentAsOne() {
        val deltas = DeltaCoalescer(limit = 100f)

        assertTrue(deltas.add("pc", 1f, 2f))
        assertFalse(deltas.add("pc", 3f, -1f))

        assertEquals(DeltaCoalescer.Delta("pc", 4f, 1f), deltas.take())
        assertNull(deltas.take())
        assertTrue(deltas.add("pc", 1f, 0f))
    }

    @Test
    fun anotherTargetDropsWhatWasPending() {
        val deltas = DeltaCoalescer(limit = 100f)

        deltas.add("a", 5f, 5f)
        deltas.add("b", 1f, 1f)

        assertEquals(DeltaCoalescer.Delta("b", 1f, 1f), deltas.take())
    }

    @Test
    fun sumsStayWithinTheLimit() {
        val deltas = DeltaCoalescer(limit = 200f)

        repeat(10) { deltas.add("pc", 50f, -50f) }

        assertEquals(DeltaCoalescer.Delta("pc", 200f, -200f), deltas.take())
    }

    @Test
    fun clearLetsTheNextDeltaScheduleAgain() {
        val deltas = DeltaCoalescer(limit = 100f)

        assertTrue(deltas.add("pc", 1f, 1f))
        deltas.clear()

        assertTrue(deltas.add("pc", 2f, 0f))
        assertEquals(DeltaCoalescer.Delta("pc", 2f, 0f), deltas.take())
    }
}
