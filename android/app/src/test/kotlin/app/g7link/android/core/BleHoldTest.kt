// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import app.g7link.android.ble.LinkFailure
import app.g7link.android.ble.LinkStage
import app.g7link.android.ble.failureAt
import app.g7link.core.BleUseState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BleHoldTest {
    @Test
    fun `out of range only when the pc was never seen`() {
        assertEquals(BleSetup.Linked, bleSetupAfterLink(null))
        assertEquals(BleSetup.OutOfRange, bleSetupAfterLink(LinkFailure.NotSeen))
        for (failure in listOf(
            LinkFailure.Connect(8),
            LinkFailure.Connect(-1),
            LinkFailure.NoService,
            LinkFailure.Subscribe,
            LinkFailure.Handshake,
            LinkFailure.TimedOut,
            LinkFailure.Refused,
        )) {
            assertEquals("$failure", BleSetup.CouldntConnect, bleSetupAfterLink(failure))
        }
    }

    @Test
    fun `a pc that says it has no window is not looked for`() {
        assertFalse(BleUseAnswer(BleUseState.OPEN, windowSecs = 0, retrySecs = 140).windowOpen)
        assertTrue(BleUseAnswer(BleUseState.OPEN, windowSecs = 12).windowOpen)
        assertTrue("an older PC doesn't say", BleUseAnswer(BleUseState.OPEN).windowOpen)
    }

    @Test
    fun `background looks back off to every five minutes`() {
        assertEquals(
            listOf(0L, 20_000L, 40_000L, 80_000L, 300_000L, 300_000L, 300_000L),
            (0..6).map { bleLookAgainAfterMs(it) },
        )
    }

    @Test
    fun `a link or trying again starts the back-off over`() {
        val backoff = BleBackoff()
        assertTrue("nothing failed yet", backoff.due(0L))
        assertEquals(20_000L, backoff.failed(1_000L))
        assertFalse(backoff.due(20_999L))
        assertTrue(backoff.due(21_000L))
        assertEquals(40_000L, backoff.failed(21_000L))
        assertEquals(80_000L, backoff.failed(61_000L))
        assertEquals(300_000L, backoff.failed(141_000L))
        assertFalse(backoff.due(440_999L))
        assertTrue(backoff.due(441_000L))
        backoff.reset()
        assertTrue(backoff.due(441_001L))
        assertEquals("from the start again", 20_000L, backoff.failed(441_001L))
    }

    @Test
    fun `a teardown says how far the attempt got`() {
        assertEquals(LinkFailure.Connect(-1), failureAt(LinkStage.Connecting, "couldn't connect"))
        assertEquals(LinkFailure.NoService, failureAt(LinkStage.Discovering, "no G7 Link service"))
        assertEquals(LinkFailure.NoService, failureAt(LinkStage.Discovering, "services didn't start"))
        assertEquals(LinkFailure.Connect(-1), failureAt(LinkStage.Discovering, "mtu didn't start"))
        assertEquals(LinkFailure.Subscribe, failureAt(LinkStage.Subscribing, "notifications refused"))
        assertEquals(LinkFailure.Subscribe, failureAt(LinkStage.Subscribing, "subscribe failed"))
        assertEquals(LinkFailure.Handshake, failureAt(LinkStage.Handshake, "write failed"))
    }
}
