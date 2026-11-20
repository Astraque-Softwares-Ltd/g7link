// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import android.content.res.Resources
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.FirstLineIcon
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.g7Tints

/**
 * A PC's agents on Home: one row in Home's row style, the Agents glyph in
 * amber, "Agents", and one line of what matters most now ([AgentsModel.homeLine]):
 * a request waiting (in the warning colour), what's running, the thread that
 * ran last, or "Start a thread". The row opens the Agents screen, where threads
 * are started and read; nothing else about Agents is on Home.
 */
@Composable
fun AgentsHomeSection(
    pc: Device,
    core: Core,
    onOpenAgents: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val all by core.agents.collectAsStateWithLifecycle()
    val state = all[pc.id] ?: AgentsState()
    val line = remember(state) { AgentsModel.homeLine(state) }
    val now = rememberNow()
    val resources = LocalResources.current
    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    val (lead, tail) = if (pc.online) {
        remember(line, now, resources, state.catalog) { homeLineText(resources, line, state, now) }
    } else {
        stringResource(R.string.not_connected) to null
    }
    val warn = pc.online && line is HomeLine.NeedsYou
    val enabled = pc.online
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onOpenAgents)
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .alpha(if (enabled) 1f else DISABLED_ALPHA),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        FirstLineIcon(icon = G7Icons.terminal, tint = tints.amber, style = MaterialTheme.typography.bodyLarge)
        Column(Modifier.weight(1f)) {
            Text(stringResource(R.string.agents_title), style = MaterialTheme.typography.bodyLarge, color = scheme.onSurface, maxLines = 1)
            Row(verticalAlignment = Alignment.CenterVertically) {
                val color = if (warn) tints.amber else scheme.onSurfaceVariant
                // The title gives way first, so how and when it ended stays readable.
                Text(
                    lead,
                    style = MaterialTheme.typography.bodySmall,
                    color = color,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                if (tail != null) {
                    Text(" · $tail", style = MaterialTheme.typography.bodySmall, color = color, maxLines = 1)
                }
            }
        }
        Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
    }
}

/**
 * The words of Home's Agents line: what leads it and, for the thread that ran
 * last, how and when it ended after the title ("Check this project out" ·
 * "done 3 hr. ago").
 */
private fun homeLineText(resources: Resources, line: HomeLine, state: AgentsState, now: Long): Pair<String, String?> = when (line) {
    is HomeLine.NeedsYou ->
        if (line.count > 1) {
            resources.getQuantityString(R.plurals.agents_home_requests, line.count, line.count) to null
        } else {
            resources.getString(R.string.agents_home_needs_you, state.agentName(line.agent)) to null
        }
    is HomeLine.Working -> when {
        line.count > 1 && line.sessions -> resources.getQuantityString(R.plurals.agents_home_sessions_working, line.count, line.count) to null
        line.count > 1 -> resources.getQuantityString(R.plurals.agents_home_threads_working, line.count, line.count) to null
        line.sinceMs != null -> resources.getString(R.string.agents_home_working_for, state.agentName(line.agent), workingTime(resources, now - line.sinceMs)) to null
        else -> resources.getString(R.string.agents_home_working, state.agentName(line.agent)) to null
    }
    is HomeLine.Finished -> {
        val ago = relativeTime(line.updatedMs, now)
        val how = when (line.status) {
            ThreadStatus.Done -> resources.getString(R.string.agents_home_done, ago)
            ThreadStatus.PlanReady -> resources.getString(R.string.agents_home_plan_ready, ago)
            ThreadStatus.Stopped -> resources.getString(R.string.agents_home_stopped, ago)
            is ThreadStatus.Failed -> resources.getString(R.string.agents_home_failed, ago)
            is ThreadStatus.Paused -> resources.getString(R.string.agents_home_paused, ago)
            // A finished thread is neither, but the line still reads.
            ThreadStatus.Working, ThreadStatus.NeedsYou -> ago
        }
        line.title to how
    }
    HomeLine.Start -> resources.getString(R.string.agents_home_start) to null
}

/** "4 min", "2 hr": how long it has worked, at a glance. */
private fun workingTime(resources: Resources, ms: Long): String {
    val minutes = (ms.coerceAtLeast(0L) / 60_000L).coerceAtLeast(1L).toInt()
    return if (minutes < 60) {
        resources.getQuantityString(R.plurals.agents_home_minutes, minutes, minutes)
    } else {
        val hours = minutes / 60
        resources.getQuantityString(R.plurals.agents_home_hours, hours, hours)
    }
}

/** How far a disabled row fades (Home's rows). */
private const val DISABLED_ALPHA = 0.45f
