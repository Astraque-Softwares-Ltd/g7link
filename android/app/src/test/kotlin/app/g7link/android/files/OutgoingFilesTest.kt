// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class OutgoingFilesTest {

    @Test
    fun `providerAuthority drops a user prefix the way Android resolves it`() {
        assertEquals("mms", OutgoingFiles.providerAuthority("mms"))
        assertEquals("mms", OutgoingFiles.providerAuthority("0@mms"))
        assertEquals("com.android.contacts", OutgoingFiles.providerAuthority("10@com.android.contacts"))
        assertEquals("media", OutgoingFiles.providerAuthority(" 10@Media "))
        assertEquals("com.example.provider", OutgoingFiles.providerAuthority("com.example.provider"))
    }

    @Test
    fun `providerAuthority refuses authorities that name no provider`() {
        assertNull(OutgoingFiles.providerAuthority(null))
        assertNull(OutgoingFiles.providerAuthority(""))
        assertNull(OutgoingFiles.providerAuthority("   "))
        assertNull(OutgoingFiles.providerAuthority("@mms"))
        assertNull(OutgoingFiles.providerAuthority("0@"))
        assertNull(OutgoingFiles.providerAuthority("user@mms"))
        assertNull(OutgoingFiles.providerAuthority("-1@mms"))
        assertNull(OutgoingFiles.providerAuthority("+0@mms"))
        assertNull(OutgoingFiles.providerAuthority("0@1@mms"))
        assertNull(OutgoingFiles.providerAuthority("99999999999@mms"))
    }

    @Test
    fun `user-qualified authorities don't get past the blocked providers`() {
        val blocked = listOf("0@mms", "10@sms", "0@mms-sms", "10@call_log", "0@com.android.contacts", "0@settings")
        for (auth in blocked) {
            assertFalse(auth, OutgoingFiles.isAllowedShareUri("content", auth, "app.g7link"))
        }
        assertFalse(OutgoingFiles.isAllowedShareUri("content", "0@app.g7link.shizuku", "app.g7link"))
        assertFalse(OutgoingFiles.isAllowedShareUri("content", "x@media", "app.g7link"))
    }

    @Test
    fun `shares from another profile still go`() {
        assertTrue(OutgoingFiles.isAllowedShareUri("content", "10@media", "app.g7link"))
        assertTrue(OutgoingFiles.isAllowedShareUri("content", "10@com.example.fileprovider", "app.g7link"))
    }
}
