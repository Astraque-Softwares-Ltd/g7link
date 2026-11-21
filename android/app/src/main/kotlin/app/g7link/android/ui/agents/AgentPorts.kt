// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.core.AgentPort

/** The dev-server line over the reply box (contract §V9.2, boards 11a and 11b). */
@Immutable
sealed interface PortLine {
    val port: Int

    /** "A server is listening on port 5173" with Open on phone: it answers on the network, and the link is one. */
    data class Open(override val port: Int, val url: String) : PortLine

    /** The same line without the button: this phone reaches the PC another way (the relay, USB). */
    data class Listening(override val port: Int) : PortLine

    /** Bound to the PC's loopback only: why it can't open here, and no button. */
    data class PcOnly(override val port: Int) : PortLine
}

/**
 * Which listening port the line names and what it offers. Nothing is probed,
 * tunnelled or forwarded: a server that answers only on the PC is explained.
 */
object AgentPorts {
    /** The port named: the first one a phone may open, else the first; null when none listens. */
    fun shown(ports: List<AgentPort>): AgentPort? = ports.firstOrNull { it.lan } ?: ports.firstOrNull()

    /**
     * The line for [ports], with [url] the address `agent_port_url` gives for
     * the shown one on this link (null off the network). None while the PC
     * can't be reached.
     */
    fun line(ports: List<AgentPort>, online: Boolean, url: (AgentPort) -> String?): PortLine? {
        if (!online) return null
        val port = shown(ports) ?: return null
        val number = port.port.toInt()
        if (!port.lan) return PortLine.PcOnly(number)
        val address = url(port)?.takeIf(::opens)
        return if (address != null) PortLine.Open(number, address) else PortLine.Listening(number)
    }

    /** Only a web address opens in the browser; anything else from the link is never offered. */
    fun opens(url: String): Boolean = url.startsWith("http://", ignoreCase = true) || url.startsWith("https://", ignoreCase = true)
}

/**
 * "A server is listening on port 5173" with Open on phone, which opens it in
 * the browser after the link sheet's confirmation; or, for one that answers on
 * the PC alone, "It only answers on Kilele itself. Start it with --host to
 * open it here." and no button.
 */
@Composable
internal fun PortLineView(line: PortLine, pcName: String, onOpen: (PortLine.Open) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 6.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(scheme.surfaceContainerHigh)
            .padding(start = 12.dp, end = 8.dp, top = if (line is PortLine.Open) 0.dp else 10.dp, bottom = if (line is PortLine.Open) 0.dp else 10.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(AgentGlyphs.server, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(15.dp))
            Text(
                stringResource(R.string.agents_port_listening, line.port),
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.weight(1f),
            )
            if (line is PortLine.Open) AccentTextButton(stringResource(R.string.agents_port_open_on_phone), onClick = { onOpen(line) })
        }
        if (line is PortLine.PcOnly) {
            Text(stringResource(R.string.agents_port_pc_only, pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
    }
}

/** A text button in the accent colour, 44 dp tall: Show beside, Open on phone. */
@Composable
internal fun AccentTextButton(label: String, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 6.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.SemiBold),
            color = MaterialTheme.colorScheme.primary,
            maxLines = 1,
        )
    }
}
