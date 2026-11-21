// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentPort
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** A dev server on the phone (contract §V9.2): only what already answers on the network, after a confirmation. */
class AgentPortsTest {
    private val lan = AgentPort(port = 5173u, lan = true)
    private val local = AgentPort(port = 3000u, lan = false)
    private val url: (AgentPort) -> String? = { "http://192.168.1.20:${it.port}/" }

    @Test
    fun `a server on the network opens on the phone through its address on this link`() {
        assertEquals(PortLine.Open(5173, "http://192.168.1.20:5173/"), AgentPorts.line(listOf(lan), online = true, url = url))
        // The first one the phone can open is named, whatever the order.
        assertEquals(PortLine.Open(5173, "http://192.168.1.20:5173/"), AgentPorts.line(listOf(local, lan), online = true, url = url))
        assertEquals(lan, AgentPorts.shown(listOf(local, lan, AgentPort(port = 8080u, lan = true))))
    }

    @Test
    fun `one that answers on the PC alone is explained, with no button`() {
        assertEquals(PortLine.PcOnly(3000), AgentPorts.line(listOf(local), online = true, url = url))
        assertEquals(PortLine.PcOnly(3000), AgentPorts.line(listOf(local, AgentPort(port = 4000u, lan = false)), online = true, url = url))
    }

    @Test
    fun `off the network (the relay, USB) the line has no button`() {
        assertEquals(PortLine.Listening(5173), AgentPorts.line(listOf(lan), online = true, url = { null }))
        // Only a web address opens: anything else from the link is not offered.
        assertEquals(PortLine.Listening(5173), AgentPorts.line(listOf(lan), online = true, url = { "file:///etc/passwd" }))
        assertEquals(PortLine.Listening(5173), AgentPorts.line(listOf(lan), online = true, url = { "intent://x" }))
        assertEquals(true, AgentPorts.opens("HTTPS://kilele.local:5173/"))
        assertEquals(false, AgentPorts.opens("javascript:alert(1)"))
    }

    @Test
    fun `nothing listens, or the PC can't be reached, no line`() {
        assertNull(AgentPorts.line(emptyList(), online = true, url = url))
        assertNull(AgentPorts.line(listOf(lan), online = false, url = url))
        assertNull(AgentPorts.shown(emptyList()))
    }
}
