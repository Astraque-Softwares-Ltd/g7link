// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.links

import app.g7link.android.R
import app.g7link.core.ClipKind
import org.junit.Assert.assertEquals
import org.junit.Test

class LinkNotificationsTest {

    @Test
    fun `every kind says what was copied`() {
        assertEquals(R.string.clip_copied_link_from, LinkNotifications.clipTitleRes(ClipKind.WEB_LINK))
        assertEquals(R.string.clip_copied_address_from, LinkNotifications.clipTitleRes(ClipKind.STREET_ADDRESS))
        assertEquals(R.string.clip_copied_phone_from, LinkNotifications.clipTitleRes(ClipKind.PHONE_NUMBER))
        assertEquals(R.string.clip_copied_tracking_from, LinkNotifications.clipTitleRes(ClipKind.TRACKING_NUMBER))
        assertEquals(R.string.clip_copied_email_from, LinkNotifications.clipTitleRes(ClipKind.EMAIL))
    }

    @Test
    fun `each button names what its intent does`() {
        assertEquals(R.string.clip_action_open_link, LinkNotifications.clipActionRes(ClipKind.WEB_LINK))
        assertEquals(R.string.clip_action_maps, LinkNotifications.clipActionRes(ClipKind.STREET_ADDRESS))
        assertEquals(R.string.clip_action_call, LinkNotifications.clipActionRes(ClipKind.PHONE_NUMBER))
        assertEquals(R.string.clip_action_track, LinkNotifications.clipActionRes(ClipKind.TRACKING_NUMBER))
        assertEquals(R.string.clip_action_write_email, LinkNotifications.clipActionRes(ClipKind.EMAIL))
    }

    @Test
    fun `a link shows without its scheme or trailing slash`() {
        assertEquals("example.com/docs", LinkNotifications.clipShownTarget(ClipKind.WEB_LINK, "https://example.com/docs/"))
        assertEquals("example.com", LinkNotifications.clipShownTarget(ClipKind.WEB_LINK, "http://example.com"))
    }

    @Test
    fun `other kinds show the copied text as it is`() {
        assertEquals("ada@example.com", LinkNotifications.clipShownTarget(ClipKind.EMAIL, "ada@example.com"))
        assertEquals("+254700000000", LinkNotifications.clipShownTarget(ClipKind.PHONE_NUMBER, "+254700000000"))
        assertEquals("1Z999AA10123456784", LinkNotifications.clipShownTarget(ClipKind.TRACKING_NUMBER, "1Z999AA10123456784"))
        assertEquals(
            "221B Baker Street, London",
            LinkNotifications.clipShownTarget(ClipKind.STREET_ADDRESS, "221B Baker Street, London"),
        )
    }

    @Test
    fun `the copied text stays on one line`() {
        assertEquals(
            "221B Baker Street, London NW1 6XE",
            LinkNotifications.clipShownTarget(ClipKind.STREET_ADDRESS, " 221B Baker Street,\nLondon\r\n NW1 6XE "),
        )
    }
}
