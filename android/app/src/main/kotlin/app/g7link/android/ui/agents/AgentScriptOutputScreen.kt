// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import kotlinx.coroutines.launch

/**
 * A run's output full-screen (phase 6 §X5.2): plain mono lines, the newest at
 * the bottom and followed as they arrive while the end is in view; "Showing
 * the last lines. The rest is on %1." when the output doesn't start at the
 * run's first line. Read whole on opening, then live from the PC's updates,
 * and read again when some went missing between them.
 */
@Composable
internal fun AgentScriptOutputScreen(device: Device, core: Core, agents: AgentsState, runId: String, onClose: () -> Unit) {
    BackHandler(onBack = onClose)
    val scheme = MaterialTheme.colorScheme
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val run = agents.scriptRuns[runId]
    var failed by remember(runId) { mutableStateOf(false) }
    LaunchedEffect(device.id, runId, device.online) {
        if (device.online) failed = core.agentScriptOutput(device.id, runId) is AgentCall.Failed
    }
    // Lines went missing between two updates: the newest are read again.
    LaunchedEffect(device.id, runId, run?.gap, device.online) {
        if (run?.gap == true && device.online) core.agentScriptOutput(device.id, runId)
    }
    val folder = run?.let { agents.scriptsFolderOf(it.threadId) }
    val canStop = run != null && AgentScripts.canStop(run, folder, offline = !device.online)
    Column(Modifier.fillMaxSize().background(scheme.surface)) {
        GitHeader(title = run?.name.orEmpty(), sub = run?.let { runStateLine(it) }, onBack = onClose)
        when {
            run == null && !device.online -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.agents_needs_pc, device.name), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            }
            run == null && failed -> Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.agents_failed_generic), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            }
            run == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp) }
            else -> {
                RunLines(run, pcName = device.name, modifier = Modifier.weight(1f))
                Box(Modifier.fillMaxWidth().height(1.dp).background(scheme.outlineVariant))
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    RunMark(run)
                    Text(
                        runWordText(AgentScripts.word(run)),
                        style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium),
                        modifier = Modifier.weight(1f),
                    )
                    if (canStop) {
                        G7RaisedButton(
                            label = stringResource(R.string.agents_script_stop),
                            onClick = {
                                scope.launch {
                                    if (core.agentScriptStop(device.id, runId) is AgentCall.Failed) core.say(resources.getString(R.string.agents_failed_generic))
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}

/** The lines, on the code ground, following the newest while the end is in view. */
@Composable
private fun RunLines(run: ScriptRun, pcName: String, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val p = agentPalette()
    val style = MaterialTheme.typography.bodySmall.mono(LocalAppFonts.current).copy(fontSize = 12.5.sp, lineHeight = 19.sp)
    val state = rememberLazyListState()
    val tail = AgentScripts.tailShown(run)
    val count = run.lines.size + if (tail) 1 else 0
    // The newest line in view as lines arrive, unless scrolled up to read: decided when a scroll ends.
    var following by remember(run.id) { mutableStateOf(true) }
    LaunchedEffect(state) {
        snapshotFlow { state.isScrollInProgress }.collect { scrolling -> if (!scrolling) following = !state.canScrollForward }
    }
    LaunchedEffect(run.lines.size, run.from) {
        if (following && count > 0) state.scrollToItem(count - 1)
    }
    LazyColumn(
        modifier.fillMaxWidth().background(scheme.surfaceContainer),
        state = state,
        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
    ) {
        if (tail) {
            item(key = "tail", contentType = "label") {
                Text(
                    stringResource(R.string.agents_script_tail, pcName),
                    style = MaterialTheme.typography.labelSmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(bottom = 6.dp),
                )
            }
        }
        itemsIndexed(run.lines, key = { index, _ -> run.from + index }, contentType = { _, _ -> "line" }) { _, line ->
            Text(line, style = style, color = p.diffText)
        }
    }
}
