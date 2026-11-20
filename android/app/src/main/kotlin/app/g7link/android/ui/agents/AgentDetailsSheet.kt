// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.core.AgentThread

/**
 * Thread details (the phone board 7H): where it runs, the model (and the one
 * that answered when another did), how it asks, how long it's been working,
 * the user's MCP servers as a count (never their names), and how full the
 * context is, as the CLI reported. Never a cost.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AgentDetailsSheet(
    thread: AgentThread,
    control: ThreadControl,
    abilities: AgentAbilities,
    pcName: String,
    now: Long,
    onDismiss: () -> Unit,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scheme = MaterialTheme.colorScheme
    val models = abilities.models
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = scheme.surfaceContainerHigh) {
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(stringResource(R.string.agents_details), style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(bottom = 8.dp))
            DetailLine(G7Icons.folder, stringResource(R.string.agents_folder_on, thread.folder, pcName))
            val model = AgentSettings.modelName(models, control.model)
            val effort = AgentSettings.effectiveEffort(models, AgentSettings.of(control))?.let(AgentSettings::effortLabel)
            DetailLine(G7Icons.sparkle, listOfNotNull(model ?: stringResource(R.string.agents_default_model), effort).joinToString(" · "))
            val answered = control.answeredModel?.takeIf { it.isNotBlank() && it != control.model }
            if (answered != null) {
                DetailLine(null, stringResource(R.string.agents_answered_by, AgentSettings.modelName(models, answered) ?: answered))
            }
            DetailLine(G7Icons.shield, modeLabel(control.mode))
            if (AgentsModel.running(thread)) DetailLine(G7Icons.clock, workingFor(thread.sinceMs, now))
            val mcp = control.mcp
            if (mcp != null && AgentUsage.mcpShown(mcp)) {
                val servers = pluralStringResource(R.plurals.agents_mcp_servers, mcp.servers, mcp.servers)
                val line = if (mcp.needLogin > 0) {
                    stringResource(R.string.agents_mcp_line, servers, pluralStringResource(R.plurals.agents_mcp_need_login, mcp.needLogin, mcp.needLogin, pcName))
                } else {
                    servers
                }
                DetailLine(AgentGlyphs.plug, line)
            }
            val context = control.context
            val percent = context?.let(AgentUsage::contextPercent)
            if (context != null && percent != null) {
                Text(
                    stringResource(R.string.agents_context, percent, AgentUsage.tokens(context.max)),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 10.dp, bottom = 6.dp),
                )
                Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(scheme.surfaceContainer)) {
                    Box(Modifier.fillMaxWidth(percent / 100f).fillMaxHeight().clip(RoundedCornerShape(2.dp)).background(scheme.onSurfaceVariant))
                }
            }
        }
    }
}

@Composable
private fun DetailLine(icon: ImageVector?, text: String) {
    val scheme = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().heightIn(min = 34.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(16.dp), contentAlignment = Alignment.Center) {
            if (icon != null) Icon(icon, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
        }
        Text(text, style = MaterialTheme.typography.bodyMedium)
    }
}
