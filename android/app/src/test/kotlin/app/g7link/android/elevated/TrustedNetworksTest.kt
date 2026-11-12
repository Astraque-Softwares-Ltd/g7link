// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.elevated

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TrustedNetworksTest {
    private val home = WifiNetwork(setOf("192.168.1.1"), setOf("fe80::1a2b:3cff:fe4d:5e6f"), "fritz.box")

    @Test
    fun `the same router and domain is the same network`() {
        assertTrue(home.matches(home.copy()))
    }

    @Test
    fun `a network with the same router address but another IPv6 router is another network`() {
        assertFalse(home.matches(home.copy(ipv6Gateways = setOf("fe80::1"))))
    }

    @Test
    fun `IPv6 not up yet doesn't decide`() {
        assertTrue(home.matches(home.copy(ipv6Gateways = emptySet())))
        assertTrue(home.copy(ipv6Gateways = emptySet()).matches(home))
    }

    @Test
    fun `another domain or router is another network`() {
        assertFalse(home.matches(home.copy(domain = null)))
        assertFalse(home.matches(home.copy(domain = "home")))
        assertFalse(home.matches(home.copy(ipv4Gateways = setOf("192.168.0.1"), ipv6Gateways = emptySet())))
    }

    @Test
    fun `nothing matches a network with no router`() {
        val unknown = WifiNetwork(emptySet(), emptySet(), null)
        assertFalse(unknown.matches(unknown))
        assertFalse(home.matches(unknown))
        assertEquals(emptyList<WifiNetwork>(), WifiNetwork.trust(emptyList(), unknown))
    }

    @Test
    fun `only IPv6 on one side and only IPv4 on the other isn't enough`() {
        val v4 = WifiNetwork(setOf("10.0.0.1"), emptySet(), null)
        val v6 = WifiNetwork(emptySet(), setOf("fe80::1"), null)
        assertFalse(v4.matches(v6))
    }

    @Test
    fun `trusting a known network again merges what was seen`() {
        val first = WifiNetwork.trust(emptyList(), home.copy(ipv6Gateways = emptySet()))
        val second = WifiNetwork.trust(first, home)
        assertEquals(listOf(home), second)
        val other = WifiNetwork(setOf("10.0.0.1"), emptySet(), null)
        assertEquals(listOf(home, other), WifiNetwork.trust(second, other))
    }

    @Test
    fun `the list keeps the latest networks, at most`() {
        val many = (1..WifiNetwork.MAX_TRUSTED + 5).fold(emptyList<WifiNetwork>()) { list, i ->
            WifiNetwork.trust(list, WifiNetwork(setOf("10.0.$i.1"), emptySet(), null))
        }
        assertEquals(WifiNetwork.MAX_TRUSTED, many.size)
        assertEquals(setOf("10.0.${WifiNetwork.MAX_TRUSTED + 5}.1"), many.last().ipv4Gateways)
    }

    @Test
    fun `the list survives being saved, and junk reads as nothing`() {
        val list = listOf(home, WifiNetwork(setOf("10.0.0.1"), emptySet(), null))
        assertEquals(list, WifiNetwork.listFromJson(WifiNetwork.listToJson(list)))
        assertEquals(emptyList<WifiNetwork>(), WifiNetwork.listFromJson("not json"))
        assertEquals(emptyList<WifiNetwork>(), WifiNetwork.listFromJson(null))
        assertEquals(emptyList<WifiNetwork>(), WifiNetwork.listFromJson("""[{"v4":[],"v6":[]}]"""))
    }

    @Test
    fun `domains and router addresses compare as networks hand them out`() {
        assertEquals("fritz.box", WifiNetwork.domainOf(" Fritz.Box lan"))
        assertEquals(null, WifiNetwork.domainOf("  "))
        assertEquals("fe80::1", WifiNetwork.gatewayOf("FE80::1%wlan0"))
        assertEquals("192.168.1.1 · fritz.box", home.address)
    }
}
