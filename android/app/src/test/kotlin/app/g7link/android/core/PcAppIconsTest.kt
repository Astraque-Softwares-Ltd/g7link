// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.core

import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PcAppIconsTest {
    @Test
    fun onlyAKeyAsThePcSendsItNamesAFile() {
        assertTrue(PcAppIcons.isKey("0123456789abcdef"))
        assertFalse("upper case", PcAppIcons.isKey("0123456789ABCDEF"))
        assertFalse("too short", PcAppIcons.isKey("0123456789abcde"))
        assertFalse("too long", PcAppIcons.isKey("0123456789abcdef0"))
        assertFalse("a path", PcAppIcons.isKey("../../abcdef0123"))
        assertFalse("empty", PcAppIcons.isKey(""))
    }

    @Test
    fun onlyAPlainPcIdNamesAFolder() {
        assertTrue(PcAppIcons.isPcFolder("ybndrfg8ejkmcpqxot1uwisza345h769"))
        assertFalse("a path", PcAppIcons.isPcFolder("../pc"))
        assertFalse("a separator", PcAppIcons.isPcFolder("pc/other"))
        assertFalse("upper case", PcAppIcons.isPcFolder("PC"))
        assertFalse("empty", PcAppIcons.isPcFolder(""))
        assertFalse("too long", PcAppIcons.isPcFolder("a".repeat(129)))
    }

    @Test
    fun oversizedIconsAreRefused() {
        assertTrue(PcAppIcons.fits(1_000, 96, 96))
        assertTrue(PcAppIcons.fits(PcAppIcons.MAX_BYTES, PcAppIcons.MAX_PX, PcAppIcons.MAX_PX))
        assertFalse("too many bytes", PcAppIcons.fits(PcAppIcons.MAX_BYTES + 1, 96, 96))
        assertFalse("too wide", PcAppIcons.fits(1_000, PcAppIcons.MAX_PX + 1, 96))
        assertFalse("too tall", PcAppIcons.fits(1_000, 96, PcAppIcons.MAX_PX + 1))
        assertFalse("undecodable: no size", PcAppIcons.fits(1_000, -1, -1))
        assertNull("empty", PcAppIcons.decode(ByteArray(0)))
        assertNull("over 16 KiB, before decoding", PcAppIcons.decode(ByteArray(PcAppIcons.MAX_BYTES + 1)))
    }
}
