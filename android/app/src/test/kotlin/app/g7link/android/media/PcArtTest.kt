// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.atomic.AtomicInteger

class PcArtTest {

    /** A decoded picture stand-in: its size in bytes is its length. */
    private class Picture(val bytes: Int)

    private fun loader(
        scope: kotlinx.coroutines.CoroutineScope,
        fetch: suspend (String, String) -> ByteArray?,
        clock: () -> Long = { 0L },
        decoded: AtomicInteger = AtomicInteger(),
        limit: Int = PcArt.FULL_BYTES,
        cache: ByteLru<String, Picture> = ByteLru(PcArt.FULL_KEPT, Long.MAX_VALUE) { it.bytes.toLong() },
    ) = ArtLoader(
        scope = scope,
        limitBytes = limit,
        cache = cache,
        fetch = fetch,
        decode = { bytes -> decoded.incrementAndGet(); Picture(bytes.size) },
        now = clock,
        log = {},
    )

    @Test
    fun `bytes over the limit are refused before decoding`() = runBlocking {
        val decoded = AtomicInteger()
        val art = loader(this, fetch = { _, _ -> ByteArray(PcArt.FULL_BYTES + 1) }, decoded = decoded)
        assertNull(art.get("pc", "big"))
        assertEquals(0, decoded.get())
        val ok = loader(this, fetch = { _, _ -> ByteArray(PcArt.FULL_BYTES) }, decoded = decoded)
        assertEquals(PcArt.FULL_BYTES, ok.get("pc", "fits")?.bytes)
        assertEquals(1, decoded.get())
    }

    @Test
    fun `the protocol's limits hold`() {
        assertEquals(2 * 1024 * 1024, PcArt.FULL_BYTES)
        assertEquals(16 * 1024, PcArt.ICON_BYTES)
        assertFalse(PcArt.fits(0, PcArt.ICON_BYTES))
        assertTrue(PcArt.fits(PcArt.ICON_BYTES, PcArt.ICON_BYTES))
        assertFalse(PcArt.fits(PcArt.ICON_BYTES + 1, PcArt.ICON_BYTES))
    }

    @Test
    fun `only well-formed keys are asked for`() {
        assertTrue(PcArt.isArtKey("0123456789abcdef"))
        assertFalse(PcArt.isArtKey(""))
        assertFalse(PcArt.isArtKey("x".repeat(65)))
        assertFalse(PcArt.isArtKey("has space"))
        assertTrue(PcArt.isIconKey("0123456789abcdef"))
        assertFalse(PcArt.isIconKey("0123456789ABCDEF"))
        assertFalse(PcArt.isIconKey("0123456789abcde"))
    }

    @Test
    fun `a huge picture decodes near the screen's size, never smaller`() {
        // A 2 MiB file can claim 20 000 px a side.
        val sample = PcArt.sampleSize(20_000, 20_000, 1080)
        assertEquals(16, sample)
        assertTrue(20_000 / sample in 1080 until 2160)
        assertEquals(1, PcArt.sampleSize(1200, 1200, 1080))
        assertEquals(1, PcArt.sampleSize(640, 640, 1080))
    }

    @Test
    fun `what decodes larger than the screen is scaled down to it`() {
        assertEquals(1080 to 1080, PcArt.fitWithin(1200, 1200, 1080))
        assertEquals(1080 to 607, PcArt.fitWithin(1250, 703, 1080))
        assertEquals(640 to 480, PcArt.fitWithin(640, 480, 1080))
        assertEquals(1 to 1080, PcArt.fitWithin(1, 4000, 1080))
    }

    @Test
    fun `the cache keeps the most recently used, by count and by bytes`() {
        val lru = ByteLru<String, Picture>(maxEntries = 4, maxBytes = 1_000) { it.bytes.toLong() }
        for (i in 1..4) lru.put("k$i", Picture(100))
        lru["k1"] // used: k2 is now the eldest
        lru.put("k5", Picture(100))
        assertNull(lru["k2"])
        assertEquals(4, lru.size)
        lru.put("k6", Picture(700))
        // 4 × 100 + 700 is over 1 000: the eldest go until it fits.
        assertTrue(lru.bytes <= 1_000)
        assertEquals(700, lru["k6"]?.bytes)
        assertEquals(100, lru["k5"]?.bytes)
        assertNull(lru["k3"])
    }

    @Test
    fun `a value larger than the whole cache isn't kept, and replacing one recounts its bytes`() {
        val lru = ByteLru<String, Picture>(maxEntries = 4, maxBytes = 1_000) { it.bytes.toLong() }
        lru.put("a", Picture(300))
        lru.put("huge", Picture(5_000))
        assertNull(lru["huge"])
        lru.put("a", Picture(500))
        assertEquals(500L, lru.bytes)
        lru.put("a", Picture(5_000))
        assertNull(lru["a"])
        assertEquals(0L, lru.bytes)
    }

    @Test
    fun `callers asking for the same key share one fetch`() = runBlocking {
        val fetches = AtomicInteger()
        val gate = CompletableDeferred<Unit>()
        val art = loader(this, fetch = { _, _ ->
            fetches.incrementAndGet()
            gate.await()
            ByteArray(10)
        })
        val first = async { art.get("pc", "key") }
        val second = async { art.get("pc", "key") }
        yield()
        yield()
        gate.complete(Unit)
        val a = first.await()
        val b = second.await()
        assertEquals(1, fetches.get())
        assertSame(a, b)
        // Once here, it's served from memory.
        assertSame(a, art.get("pc", "key"))
        assertSame(a, art.cached("pc", "key"))
        assertEquals(1, fetches.get())
    }

    @Test
    fun `a key that failed isn't asked for again for 30 s`() = runBlocking {
        var now = 0L
        val fetches = AtomicInteger()
        var fails = true
        val art = loader(this, clock = { now }, fetch = { _, _ ->
            fetches.incrementAndGet()
            if (fails) throw IllegalStateException("offline") else ByteArray(10)
        })
        assertNull(art.get("pc", "key"))
        assertEquals(1, fetches.get())
        now = PcArt.RETRY_MS - 1
        fails = false
        assertNull(art.get("pc", "key"))
        assertEquals(1, fetches.get())
        // Another key, or another PC with the same key, is its own.
        assertEquals(10, art.get("other-pc", "key")?.bytes)
        now = PcArt.RETRY_MS
        assertEquals(10, art.get("pc", "key")?.bytes)
        assertEquals(3, fetches.get())
    }

    @Test
    fun `nothing to fetch from counts as a failure`() = runBlocking {
        val fetches = AtomicInteger()
        val art = loader(this, fetch = { _, _ -> fetches.incrementAndGet(); null })
        assertNull(art.get("pc", "key"))
        assertNull(art.get("pc", "key"))
        assertEquals(1, fetches.get())
    }
}
