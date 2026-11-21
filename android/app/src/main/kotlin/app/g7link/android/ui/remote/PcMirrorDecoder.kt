// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.content.Context
import android.media.MediaCodec
import android.media.MediaFormat
import android.net.ConnectivityManager
import android.net.wifi.WifiInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface
import app.g7link.android.core.Core
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The packets on their way to the decoder, in the order the PC numbered them
 * ([offer]'s `seq`, from 0 after each config). A packet that never arrived (the
 * phone's 32-packet flow dropped it) leaves a gap, and the picture after it can't
 * be decoded: from there it drops everything until the next keyframe and asks the
 * PC for one, once per loss ([askKeyframe]). Packets wait here while the decoder
 * has no room ([poll] takes them); more than [capacity] waiting is a loss too, and
 * the waiting ones go, so the picture catches up instead of falling behind.
 */
internal class MirrorFeed<T>(
    private val capacity: Int = CAPACITY,
    private val askKeyframe: () -> Unit,
) {
    private val pending = ArrayDeque<T>()
    private var next: Long? = null
    private var waiting = true
    private var asked = false

    /** Packets the flow dropped before they got here: the gaps in the numbers. */
    var missed = 0L
        private set

    /** Packets dropped here, waiting for a keyframe or to catch up. */
    var dropped = 0L
        private set

    /** Keyframes asked for. */
    var asks = 0L
        private set

    /** How many packets wait for the decoder. */
    val size: Int get() = pending.size

    /** Packet [seq], a [keyframe] or not, arrived as [item]. */
    fun offer(seq: Long, keyframe: Boolean, item: T) {
        val expected = next
        next = seq + 1
        if (expected != null && seq != expected) {
            // Higher: packets never came. Lower: a new stream, numbered from 0 again.
            if (seq > expected) missed += seq - expected
            if (!keyframe) waiting = true
        }
        if (pending.size >= capacity) {
            // The decoder fell behind: what waits is stale.
            dropped += pending.size
            pending.clear()
            if (!keyframe) waiting = true
        }
        if (keyframe) {
            waiting = false
            asked = false
            pending.addLast(item)
            return
        }
        if (waiting) {
            dropped++
            askOnce()
            return
        }
        pending.addLast(item)
    }

    /** The next packet for the decoder, or null when none waits. */
    fun poll(): T? = pending.removeFirstOrNull()

    /**
     * A packet couldn't go to the decoder after all: the ones waiting after it
     * can't be decoded either, and the picture waits for a keyframe.
     */
    fun lost() {
        dropped += pending.size
        pending.clear()
        waiting = true
        askOnce()
    }

    /** A new stream (config): numbered from 0, and it starts with a keyframe. */
    fun restart() {
        next = null
        waiting = true
        asked = false
    }

    private fun askOnce() {
        if (asked) return
        asked = true
        asks++
        askKeyframe()
    }

    companion object {
        /** About a fifth of a second at 30 frames a second. */
        const val CAPACITY = 6
    }
}

/**
 * What the Screen tab measures each second (R0): frames shown, bytes arrived,
 * capture-to-shown latency on the PC's clock and arrived-to-shown on the phone's,
 * p50 and p95, and the packets lost and keyframes asked for.
 */
internal class MirrorStats {
    /** One second's figures. Latencies are in milliseconds, null with no samples. */
    data class Window(
        val seconds: Float,
        val frames: Int,
        val bytes: Long,
        val latencyP50: Long?,
        val latencyP95: Long?,
        val localP50: Long?,
        val localP95: Long?,
        val missed: Long,
        val dropped: Long,
        val asks: Long,
    ) {
        val fps: Float get() = if (seconds > 0f) frames / seconds else 0f
        val megabits: Float get() = if (seconds > 0f) bytes * 8f / seconds / 1_000_000f else 0f
    }

    private val lock = Any()
    private var frames = 0
    private var bytes = 0L
    private val latency = ArrayList<Long>()
    private val local = ArrayList<Long>()
    private var startedMs = -1L
    private var missed = 0L
    private var dropped = 0L
    private var asks = 0L

    /** A packet of [size] bytes arrived. */
    fun arrived(size: Int) = synchronized(lock) { bytes += size }

    /**
     * A frame showed: [captureToShownUs] after the PC captured it (null without
     * the PC's clock) and [arrivedToShownUs] after it arrived.
     */
    fun shown(captureToShownUs: Long?, arrivedToShownUs: Long?) = synchronized(lock) {
        frames++
        captureToShownUs?.takeIf { it in 0..MAX_LATENCY_US }?.let { if (latency.size < MAX_SAMPLES) latency += it }
        arrivedToShownUs?.takeIf { it in 0..MAX_LATENCY_US }?.let { if (local.size < MAX_SAMPLES) local += it }
    }

    /** The feed's running counts, as they are now. */
    fun counts(missed: Long, dropped: Long, asks: Long) = synchronized(lock) {
        this.missed = missed
        this.dropped = dropped
        this.asks = asks
    }

    private var lastMissed = 0L
    private var lastDropped = 0L
    private var lastAsks = 0L

    /** The figures since the last window at [nowMs], or null on the first call (which starts the count). */
    fun window(nowMs: Long): Window? = synchronized(lock) {
        val start = startedMs
        startedMs = nowMs
        if (start < 0L) {
            reset()
            return null
        }
        val out = Window(
            seconds = (nowMs - start).coerceAtLeast(1L) / 1000f,
            frames = frames,
            bytes = bytes,
            latencyP50 = percentile(latency, 50)?.let { it / 1000 },
            latencyP95 = percentile(latency, 95)?.let { it / 1000 },
            localP50 = percentile(local, 50)?.let { it / 1000 },
            localP95 = percentile(local, 95)?.let { it / 1000 },
            missed = missed - lastMissed,
            dropped = dropped - lastDropped,
            asks = asks - lastAsks,
        )
        reset()
        out
    }

    private fun reset() {
        frames = 0
        bytes = 0L
        latency.clear()
        local.clear()
        lastMissed = missed
        lastDropped = dropped
        lastAsks = asks
    }

    companion object {
        /** Latencies over 10 s are a clock that's off, not a frame. */
        private const val MAX_LATENCY_US = 10_000_000L
        private const val MAX_SAMPLES = 1_000

        /** The [p]th percentile of [values] (nearest rank), or null when there are none. */
        fun percentile(values: List<Long>, p: Int): Long? {
            if (values.isEmpty()) return null
            val sorted = values.sorted()
            val rank = ((p * sorted.size + 99) / 100).coerceIn(1, sorted.size)
            return sorted[rank - 1]
        }
    }
}

/**
 * The PC's screen, decoded onto [surface]: a [MediaCodec] in callback mode on a
 * thread of its own, each picture shown the moment it's decoded. Packets go in
 * through [submit] (from any thread) and wait in a [MirrorFeed] while the decoder
 * has no room, so a brief stall queues them instead of costing a keyframe.
 * [stats] hears each packet and each picture shown, and [listener] the first
 * picture after [frameWanted] was set; [clockOffsetUs] (the PC's
 * clock minus the phone's, from [Core.pcMirrorClock]) turns a packet's time into
 * the phone's.
 */
internal class PcMirrorDecoder(
    private val mime: String,
    width: Int,
    height: Int,
    private val h264: Boolean,
    surface: Surface,
    preferSoftware: Boolean,
    private val fps: Int,
    private val stats: MirrorStats,
    private val frameWanted: AtomicBoolean,
    private val listener: Listener,
) : AutoCloseable {
    interface Listener {
        /** A picture showed while [frameWanted] was set (it's cleared): the first after a (re)start. */
        fun onFirstFrame()

        /** The picture lost a packet: the PC should send a keyframe. */
        fun onKeyframeWanted()

        /** The decoder failed; it's closed. */
        fun onFailed(error: Throwable)
    }

    /** One buffer's worth for the decoder: a packet, or the parameter sets split from its front. */
    private class Chunk(val data: ByteArray, val timeUs: Long, val flags: Int, val arrivedNs: Long)

    private class Packet(val data: ByteArray, val timeUs: Long, val keyframe: Boolean, val arrivedNs: Long)

    private class Times(val arrivedNs: Long)

    // First, so a phone without the decoder starts no thread.
    private val codec: MediaCodec = createVideoDecoder(mime, preferSoftware)

    private val thread = HandlerThread("g7-pc-mirror-decoder").apply { start() }
    private val handler = Handler(thread.looper)
    private val lock = Any()
    private val feed = MirrorFeed<Packet>(askKeyframe = { listener.onKeyframeWanted() })
    private val freeInputs = ArrayDeque<Int>()
    private var carry: Chunk? = null
    private val frames = object : LinkedHashMap<Long, Times>(64, 0.75f, false) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Long, Times>?): Boolean = size > MAX_TRACKED
    }

    @Volatile private var closed = false

    /** The PC's clock minus the phone's `CLOCK_MONOTONIC`, in µs, once known. */
    @Volatile var clockOffsetUs: Long? = null

    /** The decoder's name and whether it's hardware, for the stats line. */
    val name: String = runCatching { codec.name }.getOrDefault("")
    val hardware: Boolean = runCatching {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            codec.codecInfo.isHardwareAccelerated
        } else {
            !name.startsWith("c2.android.", ignoreCase = true) && !name.startsWith("OMX.google.", ignoreCase = true)
        }
    }.getOrDefault(!preferSoftware)

    private val callback = object : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
            synchronized(lock) { freeInputs.addLast(index) }
            pump()
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            if (closed) return
            try {
                // Shown the moment it's decoded.
                codec.releaseOutputBuffer(index, true)
                if (frameWanted.compareAndSet(true, false)) listener.onFirstFrame()
            } catch (e: IllegalStateException) {
                fail(e)
            }
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) = fail(e)

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
            Log.i(TAG, "decoder output format $format")
        }
    }

    init {
        try {
            codec.setCallback(callback, handler)
            codec.setOnFrameRenderedListener({ _, presentationTimeUs, nanoTime -> rendered(presentationTimeUs, nanoTime) }, handler)
            try {
                codec.configure(format(width, height, full = true), surface, null, 0)
            } catch (_: Throwable) {
                runCatching { codec.reset() }
                codec.setCallback(callback, handler)
                codec.setOnFrameRenderedListener({ _, pts, ns -> rendered(pts, ns) }, handler)
                codec.configure(format(width, height, full = false), surface, null, 0)
            }
            // Where the system takes it (Android 11 and later): the picture's own rate, as a hint.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                runCatching { surface.setFrameRate(fps.toFloat(), Surface.FRAME_RATE_COMPATIBILITY_DEFAULT) }
            }
            codec.start()
        } catch (e: Throwable) {
            close()
            throw e
        }
    }

    private fun format(width: Int, height: Int, full: Boolean): MediaFormat =
        MediaFormat.createVideoFormat(mime, width, height).apply {
            if (!full) return@apply
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            // What the PC's encoder writes (BT.709, limited range), so the decoder needn't guess.
            setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709)
            setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED)
            setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO)
        }

    /** A packet arrived from the PC. */
    fun submit(packet: Core.PcMirrorPacket) {
        if (closed) return
        stats.arrived(packet.data.size)
        synchronized(lock) {
            feed.offer(packet.seq, packet.keyframe, Packet(packet.data, packet.timeUs, packet.keyframe, packet.arrivedNs))
            stats.counts(feed.missed, feed.dropped, feed.asks)
        }
        handler.post(::pump)
    }

    /** Fills every free input buffer with what waits. Runs on the decoder's thread. */
    private fun pump() {
        if (closed) return
        while (true) {
            val index: Int
            val unit: Chunk
            synchronized(lock) {
                if (freeInputs.isEmpty()) return
                val carried = carry
                carry = null
                unit = carried ?: feed.poll()?.let(::split) ?: return
                index = freeInputs.removeFirst()
            }
            if (!queue(index, unit)) return
        }
    }

    /**
     * Splits an H.264 keyframe's parameter sets off its front, for decoders that
     * want them as codec config; the rest waits as [carry] for the next buffer.
     */
    private fun split(packet: Packet): Chunk {
        val flags = if (packet.keyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
        if (packet.keyframe && h264) {
            extractH264CodecConfig(packet.data)?.let { (csd, slice) ->
                carry = Chunk(slice, packet.timeUs, MediaCodec.BUFFER_FLAG_KEY_FRAME, packet.arrivedNs)
                return Chunk(csd, packet.timeUs, MediaCodec.BUFFER_FLAG_CODEC_CONFIG, packet.arrivedNs)
            }
        }
        return Chunk(packet.data, packet.timeUs, flags, packet.arrivedNs)
    }

    /** Puts [unit] in input buffer [index]; false once the decoder has failed. */
    private fun queue(index: Int, unit: Chunk): Boolean {
        try {
            val buffer = codec.getInputBuffer(index)
            if (buffer == null || unit.data.size > buffer.capacity()) {
                // Too large for the decoder: the buffer goes back, the picture waits for a keyframe.
                synchronized(lock) {
                    freeInputs.addFirst(index)
                    if ((unit.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0) carry = null
                    feed.lost()
                }
                Log.w(TAG, "a ${unit.data.size}-byte packet is larger than the decoder takes")
                return true
            }
            buffer.clear()
            buffer.put(unit.data)
            if ((unit.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                synchronized(lock) { frames[unit.timeUs] = Times(unit.arrivedNs) }
            }
            codec.queueInputBuffer(index, 0, unit.data.size, unit.timeUs, unit.flags)
            return true
        } catch (e: IllegalStateException) {
            fail(e)
            return false
        }
    }

    /** The picture taken at [presentationTimeUs] (the PC's clock) showed at [nanoTime] (`System.nanoTime`). */
    private fun rendered(presentationTimeUs: Long, nanoTime: Long) {
        if (closed) return
        val times = synchronized(lock) { frames.remove(presentationTimeUs) }
        val shownUs = nanoTime / 1000
        val offset = clockOffsetUs
        stats.shown(
            captureToShownUs = offset?.let { shownUs - (presentationTimeUs - it) },
            arrivedToShownUs = times?.let { (nanoTime - it.arrivedNs) / 1000 },
        )
    }

    private fun fail(error: Throwable) {
        if (closed) return
        Log.w(TAG, "decoder failed ($name)", error)
        close()
        listener.onFailed(error)
    }

    override fun close() {
        if (closed) return
        closed = true
        runCatching { codec.stop() }
        runCatching { codec.release() }
        thread.quitSafely()
    }

    companion object {
        private const val TAG = "PcMirrorDecoder"
        private const val MAX_TRACKED = 64
    }
}

/**
 * Says at the start of a stream what it goes over (R0): the Wi-Fi band, link
 * speed and signal, and the display's refresh rate.
 */
@Suppress("DEPRECATION")
internal fun logStreamStart(context: Context, refreshRate: Float?, request: String) {
    val wifi: WifiInfo? = runCatching {
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        val fromNetwork = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            connectivity?.getNetworkCapabilities(connectivity.activeNetwork)?.transportInfo as? WifiInfo
        } else {
            null
        }
        fromNetwork ?: context.applicationContext.getSystemService(WifiManager::class.java)?.connectionInfo
    }.getOrNull()
    val band = wifi?.frequency?.let { mhz ->
        when {
            mhz in 2400..2500 -> "2.4 GHz"
            mhz in 4900..5900 -> "5 GHz"
            mhz in 5925..7125 -> "6 GHz"
            mhz > 0 -> "$mhz MHz"
            else -> "?"
        }
    } ?: "none"
    val speeds = if (wifi != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        "link ${wifi.linkSpeed} Mbit/s (rx ${wifi.rxLinkSpeedMbps}, tx ${wifi.txLinkSpeedMbps})"
    } else {
        "link ${wifi?.linkSpeed ?: -1} Mbit/s"
    }
    Log.i(
        "PcMirrorStats",
        "start $request; wifi $band, $speeds, rssi ${wifi?.rssi ?: 0} dBm; display ${refreshRate ?: 0f} Hz",
    )
}

/**
 * Extracts leading H.264 SPS (NAL type 7) and PPS (NAL type 8) parameter sets from an Annex-B
 * keyframe so hardware decoders on Qualcomm/Exynos/MediaTek receive explicit `BUFFER_FLAG_CODEC_CONFIG`.
 */
internal fun extractH264CodecConfig(data: ByteArray): Pair<ByteArray, ByteArray>? {
    var i = 0
    var firstPlayableNalOffset = -1
    var sawSpsOrPps = false
    while (i + 4 < data.size) {
        val scLen = if (data[i].toInt() == 0 && data[i + 1].toInt() == 0 && data[i + 2].toInt() == 1) {
            3
        } else if (i + 4 < data.size && data[i].toInt() == 0 && data[i + 1].toInt() == 0 && data[i + 2].toInt() == 0 && data[i + 3].toInt() == 1) {
            4
        } else {
            i++
            continue
        }
        val nalHeaderIdx = i + scLen
        if (nalHeaderIdx >= data.size) break
        val nalType = data[nalHeaderIdx].toInt() and 0x1F
        if (nalType == 7 || nalType == 8) {
            sawSpsOrPps = true
        } else if (nalType == 5 || nalType == 1) {
            firstPlayableNalOffset = i
            break
        }
        i = nalHeaderIdx + 1
    }
    return if (sawSpsOrPps && firstPlayableNalOffset > 0 && firstPlayableNalOffset < data.size) {
        Pair(
            data.copyOfRange(0, firstPlayableNalOffset),
            data.copyOfRange(firstPlayableNalOffset, data.size),
        )
    } else {
        null
    }
}

/** A decoder for [mime]: the phone's own (hardware), or with [preferSoftware], Android's software one. */
internal fun createVideoDecoder(mime: String, preferSoftware: Boolean): MediaCodec {
    val defaultCodec = runCatching { MediaCodec.createDecoderByType(mime) }.getOrNull()
    val isGoldfish = defaultCodec?.name?.startsWith("c2.goldfish.", ignoreCase = true) == true
    if (!preferSoftware && defaultCodec != null && !isGoldfish) {
        return defaultCodec
    }
    runCatching { defaultCodec?.release() }
    val candidates = when (mime) {
        MediaFormat.MIMETYPE_VIDEO_AVC -> listOf("c2.android.avc.decoder", "OMX.google.h264.decoder")
        MediaFormat.MIMETYPE_VIDEO_HEVC -> listOf("c2.android.hevc.decoder", "OMX.google.hevc.decoder")
        "video/av01" -> listOf("c2.android.av1.decoder", "c2.android.av1-dav1d.decoder")
        else -> emptyList()
    }
    for (name in candidates) {
        val codec = runCatching { MediaCodec.createByCodecName(name) }.getOrNull()
        if (codec != null) {
            return codec
        }
    }
    return MediaCodec.createDecoderByType(mime)
}
