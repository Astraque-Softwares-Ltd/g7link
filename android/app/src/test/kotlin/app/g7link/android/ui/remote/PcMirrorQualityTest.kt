// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import org.junit.Assert.assertEquals
import org.junit.Test

class PcMirrorQualityTest {
    private val avc = "video/avc"
    private val hevc = "video/hevc"

    @Test
    fun theDataSaverAsksForWhatThe720pSettingDid() {
        assertEquals(PcMirrorQuality.Request(maxSize = 1280, bitrate = 4_000_000), PcMirrorQuality.request(dataSaver = true))
        // Whatever the decoder takes.
        assertEquals(PcMirrorQuality.request(dataSaver = true), PcMirrorQuality.request(dataSaver = true, decoderMaxEdge = 1920))
    }

    @Test
    fun sharpIsTheDefaultAndAsksForUpTo2560() {
        val sharp = PcMirrorQuality.request(dataSaver = false)
        assertEquals(2560, sharp.maxSize)
        // A ceiling: the PC gives a 1080p screen about 8 Mbit/s, 1440p about 14.
        assertEquals(16_000_000, sharp.bitrate)
    }

    @Test
    fun aDecoderThatCantTake2560GetsAt1920() {
        assertEquals(1920, PcMirrorQuality.request(dataSaver = false, decoderMaxEdge = 1920).maxSize)
        assertEquals(1920, PcMirrorQuality.request(dataSaver = false, decoderMaxEdge = 1080).maxSize)
    }

    @Test
    fun theDecoderMustTakeEverySizeA2560ScreenComesAt() {
        assertEquals(2560, PcMirrorQuality.decoderMaxEdge(listOf(avc, hevc)) { _, _, _ -> true })
        // A 4096 x 2304 decoder takes 16:10 at 2560 too.
        assertEquals(2560, PcMirrorQuality.decoderMaxEdge(listOf(avc)) { _, w, h -> w <= 4096 && h <= 2304 })
        // A 1080p-only decoder.
        assertEquals(1920, PcMirrorQuality.decoderMaxEdge(listOf(avc)) { _, w, h -> w <= 1920 && h <= 1088 })
        // One that takes 1440p but not 1600 lines.
        assertEquals(1920, PcMirrorQuality.decoderMaxEdge(listOf(avc)) { _, _, h -> h <= 1440 })
        // HEVC is what the PC sends when it can: its decoder counts as much.
        assertEquals(1920, PcMirrorQuality.decoderMaxEdge(listOf(avc, hevc)) { mime, _, _ -> mime == avc })
    }
}
