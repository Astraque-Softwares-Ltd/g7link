// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import org.junit.Assert.assertEquals
import org.junit.Test

class PcCaptureFormatTest {
    @Test
    fun `a recording's length reads as a clock`() {
        assertEquals("0:00", PcCaptureFormat.elapsed(0))
        assertEquals("0:00", PcCaptureFormat.elapsed(-5_000))
        assertEquals("0:42", PcCaptureFormat.elapsed(42_999))
        assertEquals("12:05", PcCaptureFormat.elapsed((12 * 60 + 5) * 1_000L))
        assertEquals("1:02:03", PcCaptureFormat.elapsed((3_600 + 2 * 60 + 3) * 1_000L))
    }
}
