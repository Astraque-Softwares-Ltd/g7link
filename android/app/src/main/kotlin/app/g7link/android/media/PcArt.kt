// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.media

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.Log
import androidx.compose.runtime.Immutable
import androidx.core.graphics.scale
import app.g7link.android.ui.player.CoverLight
import app.g7link.android.ui.player.PlayerColors
import app.g7link.core.FfiThemeScheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.withContext

/**
 * The pictures a PC's player names without sending them (docs/protocol/media.md): the large
 * artwork for the full player (`media.art`) and the player app's icon (`media.icon`), fetched
 * when first shown and kept in memory only, by "pc/key". One fetch per key at a time; a key
 * that failed isn't asked for again for [RETRY_MS].
 */
class PcArt internal constructor(
    scope: CoroutineScope,
    fetchArt: suspend (pc: String, key: String) -> ByteArray?,
    fetchIcon: suspend (pc: String, key: String) -> ByteArray?,
    /** The screen's shorter side in pixels: no full picture is decoded larger. */
    private val screenSide: Int,
) {
    private val fullPictures = ArtLoader<Bitmap>(
        scope = scope,
        limitBytes = FULL_BYTES,
        cache = ByteLru<String, Bitmap>(FULL_KEPT, FULL_KEPT * 4L * screenSide.coerceAtLeast(1) * screenSide.coerceAtLeast(1)) {
            it.allocationByteCount.toLong()
        },
        fetch = fetchArt,
        decode = { decodeWithin(it, screenSide) },
    )
    private val icons = ArtLoader<Bitmap>(
        scope = scope,
        limitBytes = ICON_BYTES,
        cache = ByteLru<String, Bitmap>(ICONS_KEPT, ICONS_KEPT * 4L * ICON_PX * ICON_PX) { it.allocationByteCount.toLong() },
        fetch = fetchIcon,
        decode = { decodeWithin(it, ICON_PX) },
    )

    /** PC [pc]'s large picture named [key] (a player's `art_full`), or null when it can't be had. */
    suspend fun full(pc: String, key: String): Bitmap? = if (isArtKey(key)) fullPictures.get(pc, key) else null

    /** The large picture if it's already here, for the first frame. */
    fun fullNow(pc: String, key: String): Bitmap? = if (isArtKey(key)) fullPictures.cached(pc, key) else null

    /** PC [pc]'s app icon named [key] (a player's `icon`), or null when it can't be had. */
    suspend fun icon(pc: String, key: String): Bitmap? = if (isIconKey(key)) icons.get(pc, key) else null

    /** The app icon if it's already here. */
    fun iconNow(pc: String, key: String): Bitmap? = if (isIconKey(key)) icons.cached(pc, key) else null

    companion object {
        /** The largest picture `media.art` may carry (`media_limits::ART_FULL_BYTES`). */
        const val FULL_BYTES = 2 * 1024 * 1024

        /** The largest icon `media.icon` may carry (`media_limits::ICON_BYTES`). */
        const val ICON_BYTES = 16 * 1024

        const val FULL_KEPT = 4
        const val ICONS_KEPT = 32

        /** Icons are decoded to at most this many pixels a side. */
        const val ICON_PX = 128

        /** How long a key that failed is left alone. */
        const val RETRY_MS = 30_000L

        /** An `art_full` key as the protocol allows it: 1 to 64 bytes, printable ASCII. */
        fun isArtKey(key: String): Boolean = key.length in 1..64 && key.all { it in '!'..'~' }

        /** An `icon` key: 16 lowercase hex digits. */
        fun isIconKey(key: String): Boolean = key.length == 16 && key.all { it in '0'..'9' || it in 'a'..'f' }

        /** Whether [size] bytes may be decoded at all under [limit]. */
        fun fits(size: Int, limit: Int): Boolean = size in 1..limit

        /** The power-of-two sample size for a [width] × [height] picture shown at most [maxSide] a side. */
        fun sampleSize(width: Int, height: Int, maxSide: Int): Int = sampleSizeToCover(width, height, maxSide, maxSide)

        /** [width] × [height] scaled down (never up) to fit [maxSide] on its longer side. */
        fun fitWithin(width: Int, height: Int, maxSide: Int): Pair<Int, Int> {
            if (width <= 0 || height <= 0 || maxSide <= 0) return width to height
            if (maxOf(width, height) <= maxSide) return width to height
            return if (width >= height) {
                maxSide to (height.toLong() * maxSide / width).toInt().coerceAtLeast(1)
            } else {
                (width.toLong() * maxSide / height).toInt().coerceAtLeast(1) to maxSide
            }
        }

        /**
         * Decodes [bytes] no larger than [maxSide] a side, whatever size the picture claims: a
         * small file can describe a huge image. Null for anything that isn't a picture.
         */
        private fun decodeWithin(bytes: ByteArray, maxSide: Int): Bitmap? = try {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
                null
            } else {
                val options = BitmapFactory.Options().apply { inSampleSize = sampleSize(bounds.outWidth, bounds.outHeight, maxSide) }
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.let { decoded ->
                    val (w, h) = fitWithin(decoded.width, decoded.height, maxSide)
                    if (w == decoded.width && h == decoded.height) {
                        decoded
                    } else {
                        decoded.scale(w, h).also { if (it !== decoded) decoded.recycle() }
                    }
                }
            }
        } catch (e: OutOfMemoryError) {
            Log.w(TAG, "a PC picture was too large to decode")
            null
        } catch (e: RuntimeException) {
            null
        }

        private const val TAG = "PcArt"
    }
}

/**
 * Pictures by "pc/key", fetched and decoded once: callers asking for a key being fetched wait
 * for that fetch, a key that failed isn't fetched again for [retryMs], and what arrives is kept
 * in [cache]. Bytes over [limitBytes] are refused before decoding.
 */
internal class ArtLoader<V : Any>(
    private val scope: CoroutineScope,
    private val limitBytes: Int,
    private val cache: ByteLru<String, V>,
    private val fetch: suspend (pc: String, key: String) -> ByteArray?,
    private val decode: (ByteArray) -> V?,
    private val now: () -> Long = { android.os.SystemClock.elapsedRealtime() },
    private val retryMs: Long = PcArt.RETRY_MS,
    private val log: (String) -> Unit = { Log.i("PcArt", it) },
) {
    private val lock = Any()
    private val loading = HashMap<String, Deferred<V?>>()
    private val failedAt = LinkedHashMap<String, Long>()

    fun cached(pc: String, key: String): V? = synchronized(lock) { cache[id(pc, key)] }

    suspend fun get(pc: String, key: String): V? {
        val id = id(pc, key)
        val job = synchronized(lock) {
            cache[id]?.let { return it }
            val failed = failedAt[id]
            if (failed != null) {
                if (now() - failed < retryMs) return null
                failedAt.remove(id)
            }
            loading.getOrPut(id) { scope.async(start = CoroutineStart.LAZY) { load(pc, key, id) } }
        }
        job.start()
        return job.await()
    }

    private suspend fun load(pc: String, key: String, id: String): V? {
        val value = try {
            val bytes = fetch(pc, key)
            if (bytes == null || !PcArt.fits(bytes.size, limitBytes)) null else withContext(Dispatchers.Default) { decode(bytes) }
        } catch (e: CancellationException) {
            synchronized(lock) { loading.remove(id) }
            throw e
        } catch (e: Exception) {
            // Never the key or the bytes: the key can name what plays.
            log("a PC picture wasn't fetched: ${e.javaClass.simpleName}")
            null
        }
        synchronized(lock) {
            loading.remove(id)
            if (value != null) {
                cache.put(id, value)
            } else {
                failedAt.remove(id)
                failedAt[id] = now()
                // Keep the list of failures short: the oldest have long expired.
                while (failedAt.size > MAX_FAILURES) failedAt.remove(failedAt.keys.first())
            }
        }
        return value
    }

    private fun id(pc: String, key: String) = "$pc/$key"

    private companion object {
        const val MAX_FAILURES = 64
    }
}

/**
 * A least-recently-used map that keeps at most [maxEntries] values and [maxBytes] of them by
 * [sizeOf]. A value larger than [maxBytes] on its own isn't kept. Not thread-safe.
 */
internal class ByteLru<K : Any, V : Any>(
    private val maxEntries: Int,
    private val maxBytes: Long,
    private val sizeOf: (V) -> Long,
) {
    private val map = LinkedHashMap<K, V>(16, 0.75f, true)

    var bytes = 0L
        private set

    val size: Int get() = map.size

    operator fun get(key: K): V? = map[key]

    fun put(key: K, value: V) {
        val size = sizeOf(value)
        map.remove(key)?.let { bytes -= sizeOf(it) }
        if (size > maxBytes) return
        map[key] = value
        bytes += size
        val eldest = map.entries.iterator()
        while ((map.size > maxEntries || bytes > maxBytes) && eldest.hasNext()) {
            val entry = eldest.next()
            bytes -= sizeOf(entry.value)
            eldest.remove()
        }
    }
}

/**
 * What the player takes from a cover besides the picture: the extractor's colours ([scheme],
 * null if it found none), the light under the full player's header ([light]), and the mini
 * player's blurred backdrop with how far to dim it ([backdrop], [backdropDim]).
 */
@Immutable
class ArtColors(
    val scheme: FfiThemeScheme?,
    val light: CoverLight?,
    val backdrop: Bitmap?,
    val backdropDim: Float,
)

/**
 * The [ArtColors] of a decoded cover, from one ≤112 px sample: the same sample and cache key
 * the seed colour has always come from (`extractArtworkScheme`), so the seed is unchanged.
 * Runs off the main thread; null if the picture can't be read.
 */
internal fun artColors(bitmap: Bitmap, cacheKey: String?): ArtColors? = try {
    val w0 = bitmap.width
    val h0 = bitmap.height
    if (w0 <= 0 || h0 <= 0) {
        null
    } else {
        val scale = minOf(1f, SAMPLE_SIDE.toFloat() / maxOf(w0, h0))
        val w = (w0 * scale).toInt().coerceAtLeast(1)
        val h = (h0 * scale).toInt().coerceAtLeast(1)
        val sampled = if (w != w0 || h != h0) bitmap.scale(w, h) else bitmap
        val pixels = IntArray(w * h)
        sampled.getPixels(pixels, 0, w, 0, 0, w, h)
        if (sampled !== bitmap) sampled.recycle()

        val rgba = ByteArray(w * h * 4)
        var out = 0
        for (px in pixels) {
            rgba[out] = ((px ushr 16) and 0xFF).toByte()
            rgba[out + 1] = ((px ushr 8) and 0xFF).toByte()
            rgba[out + 2] = (px and 0xFF).toByte()
            rgba[out + 3] = ((px ushr 24) and 0xFF).toByte()
            out += 4
        }
        val scheme = try {
            app.g7link.core.extractArtworkScheme(rgba, w.toUInt(), h.toUInt(), cacheKey)
        } catch (e: Exception) {
            null
        }
        val (cols, rows) = PlayerColors.backdropSize(w, h)
        val blocks = PlayerColors.blockAverages(pixels, w, h, cols, rows)
        val backdrop = if (blocks.isEmpty()) null else Bitmap.createBitmap(blocks, cols, rows, Bitmap.Config.ARGB_8888)
        ArtColors(
            scheme = scheme,
            light = PlayerColors.measure(pixels, w, h),
            backdrop = backdrop,
            backdropDim = PlayerColors.miniDim(PlayerColors.brightest(blocks)),
        )
    }
} catch (e: Exception) {
    null
} catch (e: OutOfMemoryError) {
    null
}

/** The extractor's sample: the same ≤112 px the seed colour uses (`extractArtworkSeedColor`). */
private const val SAMPLE_SIDE = 112
