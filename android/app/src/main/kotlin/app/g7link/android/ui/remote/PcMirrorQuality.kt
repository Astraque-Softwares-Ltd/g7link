// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.media.MediaCodecList
import android.media.MediaFormat

/**
 * What the Screen tab asks the PC for. Sharp, the default: the PC's screen at
 * its own size up to a 2560 px side, never scaled up, with the bitrate the PC
 * picks from that size, up to [SHARP_BITRATE]. The data saver: what the old
 * 720p setting asked, 1280 px at 4 Mbit/s.
 */
object PcMirrorQuality {
    const val SHARP_MAX_SIZE = 2560
    const val SHARP_BITRATE = 16_000_000
    /** The longest side asked for when this phone's decoder can't take 2560. */
    const val DECODER_FALLBACK_SIZE = 1920
    const val SAVER_MAX_SIZE = 1280
    const val SAVER_BITRATE = 4_000_000
    const val FPS = 30

    data class Request(val maxSize: Int, val bitrate: Int)

    /** What to ask for, sharp or with the [dataSaver], when the decoder takes up to [decoderMaxEdge]. */
    fun request(dataSaver: Boolean, decoderMaxEdge: Int = SHARP_MAX_SIZE): Request =
        if (dataSaver) {
            Request(SAVER_MAX_SIZE, SAVER_BITRATE)
        } else {
            Request(SHARP_MAX_SIZE.coerceAtMost(decoderMaxEdge).coerceAtLeast(DECODER_FALLBACK_SIZE), SHARP_BITRATE)
        }

    /** The sizes a screen with a 2560 px side comes at: 16:10, 16:9 and 21:9. */
    internal val SHARP_SIZES = listOf(2560 to 1600, 2560 to 1440, 2560 to 1080)

    /**
     * The longest side to ask for, given whether the decoder for each codec
     * the PC may send ([supports], per MIME type) takes a size: 2560 when it
     * takes every size such a screen comes at, else [DECODER_FALLBACK_SIZE].
     */
    fun decoderMaxEdge(mimes: List<String>, supports: (mime: String, width: Int, height: Int) -> Boolean): Int =
        if (mimes.all { mime -> SHARP_SIZES.all { (w, h) -> supports(mime, w, h) } }) {
            SHARP_MAX_SIZE
        } else {
            DECODER_FALLBACK_SIZE
        }
}

/** What this phone's video decoders take, as the Screen tab picks them. */
internal object PcMirrorDecoders {
    /**
     * The longest side the decoders the Screen tab would open take, for H.264
     * and, where there's a decoder for it, HEVC. Reading the codec list can take
     * a moment the first time: not on the main thread.
     */
    val maxEdge: Int by lazy {
        val infos = runCatching { MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.toList() }
            .getOrDefault(emptyList())
        // The decoder createDecoderByType opens: the first for the type. The
        // emulator's is passed over for the software one, as it is when opened.
        fun decoderFor(mime: String) = infos.firstOrNull { info ->
            !info.isEncoder &&
                info.supportedTypes.any { it.equals(mime, ignoreCase = true) } &&
                !info.name.startsWith("c2.goldfish.", ignoreCase = true)
        }
        val mimes = listOf(MediaFormat.MIMETYPE_VIDEO_AVC, MediaFormat.MIMETYPE_VIDEO_HEVC)
            .filter { decoderFor(it) != null }
        if (mimes.isEmpty()) {
            PcMirrorQuality.DECODER_FALLBACK_SIZE
        } else {
            PcMirrorQuality.decoderMaxEdge(mimes) { mime, width, height ->
                runCatching {
                    decoderFor(mime)?.getCapabilitiesForType(mime)?.videoCapabilities?.isSizeSupported(width, height)
                }.getOrNull() == true
            }
        }
    }
}
