// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.mono
import kotlinx.coroutines.launch

/**
 * A thread's Scripts (phase 6 §X5.2), from its menu or the running line: the
 * folder's scripts with Run while the PC lets phones run them, then the
 * thread's runs, each opening its output. Run goes through a confirmation
 * showing the command each time; nothing is sent before its Run.
 */
@Composable
internal fun AgentScriptsSheet(
    device: Device,
    core: Core,
    agents: AgentsState,
    threadId: String,
    readonly: Boolean,
    onOutput: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val offline = !device.online
    val folder = agents.scriptsFolderOf(threadId)
    val runs = agents.runsOf(threadId)
    val sheet = AgentScripts.sheet(folder, runs, offline, readonly)
    var ask by remember(threadId) { mutableStateOf<RunAsk>(RunAsk.None) }
    // The runs as the PC keeps them, with who started each, each time the sheet opens.
    LaunchedEffect(device.id, threadId, device.online) {
        if (device.online) core.agentScriptRuns(device.id, threadId)
    }
    val stop: (ScriptRun) -> Unit = { run ->
        scope.launch {
            if (core.agentScriptStop(device.id, run.id) is AgentCall.Failed) core.say(resources.getString(R.string.agents_failed_generic))
        }
    }
    val run: () -> Unit = {
        val (next, script) = AgentScriptAsk.confirm(ask)
        ask = next
        if (script != null) {
            scope.launch {
                val started = core.agentScriptRun(device.id, threadId, script.id)
                ask = AgentScriptAsk.done()
                when (started) {
                    is ScriptStart.Started -> {
                        if (AgentScripts.word(started.run) == RunWord.NotStarted) core.say(resources.getString(R.string.agents_script_not_started))
                        onOutput(started.run.id)
                    }
                    is ScriptStart.Refused -> core.say(
                        when (started.why) {
                            ScriptRefusal.NEEDS_PC -> resources.getString(R.string.agents_needs_pc, device.name)
                            ScriptRefusal.SWITCH_OFF -> resources.getString(R.string.agents_script_off, device.name)
                            ScriptRefusal.BUSY -> resources.getString(R.string.agents_script_busy, script.name)
                            ScriptRefusal.LIMIT -> resources.getString(R.string.agents_script_limit, device.name)
                            ScriptRefusal.GONE -> resources.getString(R.string.agents_script_gone)
                            ScriptRefusal.FAILED -> resources.getString(R.string.agents_failed_generic)
                        },
                    )
                }
            }
        }
    }

    when (val shown = ask) {
        is RunAsk.Confirm -> {
            ScriptConfirm(shown.script, pcName = device.name, sending = false, onRun = run, onDismiss = { ask = AgentScriptAsk.cancel(ask) })
            return
        }
        is RunAsk.Sending -> {
            ScriptConfirm(shown.script, pcName = device.name, sending = true, onRun = {}, onDismiss = { ask = AgentScriptAsk.cancel(ask) })
            return
        }
        RunAsk.None -> Unit
    }

    AgentSheet(onDismiss) {
        val scheme = MaterialTheme.colorScheme
        Column(
            Modifier.fillMaxWidth().heightIn(max = 560.dp).verticalScroll(rememberScrollState()).padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(stringResource(R.string.agents_script_title), style = MaterialTheme.typography.titleLarge)
            agents.thread(threadId)?.folder?.takeIf { it.isNotBlank() }?.let { label ->
                Text(stringResource(R.string.agents_folder_on, label, device.name), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            when (sheet.note) {
                ScriptsNote.NEEDS_PC -> GitNotice(stringResource(R.string.agents_needs_pc, device.name))
                ScriptsNote.SWITCH_OFF -> GitNotice(stringResource(R.string.agents_script_off, device.name))
                null -> Unit
            }
            for (row in sheet.scripts) {
                ScriptRowView(
                    row = row,
                    onRun = { ask = AgentScriptAsk.tap(ask, row) },
                    onStop = { row.running?.let(stop) },
                )
            }
            if (sheet.scripts.isNotEmpty()) Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).height(1.dp).background(scheme.outlineVariant))
            if (sheet.runs.isEmpty()) {
                Text(
                    stringResource(R.string.agents_script_none),
                    style = MaterialTheme.typography.bodyMedium,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 8.dp),
                )
            }
            for (each in sheet.runs) {
                RunRowView(
                    run = each,
                    canStop = each.id in sheet.stoppable,
                    onOpen = { onOutput(each.id) },
                    onStop = { stop(each) },
                )
            }
        }
    }
}

/** A script: its name, its command in mono, and Run (or a ring and Stop while it runs). */
@Composable
private fun ScriptRowView(row: ScriptRow, onRun: () -> Unit, onStop: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(row.script.name, style = MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Medium), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                row.script.command,
                style = MaterialTheme.typography.bodySmall.mono(LocalAppFonts.current),
                color = scheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (row.running != null) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
            if (row.canStop) G7RowAction(label = stringResource(R.string.agents_script_stop), onClick = onStop)
        } else {
            G7RowAction(label = stringResource(R.string.agents_script_run), onClick = onRun, accent = true, enabled = row.canRun)
        }
    }
}

/** A run: its name, its state ("from %1" for another phone's), Stop when this phone may stop it; a tap opens its output. */
@Composable
private fun RunRowView(run: ScriptRun, canStop: Boolean, onOpen: () -> Unit, onStop: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onOpen)
            .padding(horizontal = 4.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        RunMark(run)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(run.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(runStateLine(run), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (canStop) G7RowAction(label = stringResource(R.string.agents_script_stop), onClick = onStop)
        Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
    }
}

/** A ring while it runs; a check when it finished; an x when it failed; nothing when it was stopped. */
@Composable
internal fun RunMark(run: ScriptRun) {
    Box(Modifier.size(16.dp), contentAlignment = Alignment.Center) {
        when (AgentScripts.word(run)) {
            RunWord.Running -> CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
            RunWord.Done -> CheckMark(CheckState.PASS, size = 15.dp)
            is RunWord.FailedExit, RunWord.Failed, RunWord.NotStarted -> CheckMark(CheckState.FAIL, size = 15.dp)
            RunWord.Stopped -> Unit
        }
    }
}

/** A run's state in words, with "from %1" when another phone started it. */
@Composable
internal fun runStateLine(run: ScriptRun): String {
    val state = runWordText(AgentScripts.word(run))
    val from = AgentScripts.fromPhone(run)?.let { stringResource(R.string.agents_script_from, it) }
    return listOfNotNull(state, from).joinToString(" · ")
}

@Composable
internal fun runWordText(word: RunWord): String = when (word) {
    RunWord.Running -> stringResource(R.string.agents_script_state_running)
    RunWord.Done -> stringResource(R.string.agents_script_state_done)
    is RunWord.FailedExit -> stringResource(R.string.agents_script_state_failed_exit, word.exit)
    RunWord.Failed -> stringResource(R.string.agents_script_state_failed)
    RunWord.Stopped -> stringResource(R.string.agents_script_state_stopped)
    RunWord.NotStarted -> stringResource(R.string.agents_script_not_started)
}

/** "Run “dev” on Kilele?", the command as it will run, Cancel and Run. */
@Composable
private fun ScriptConfirm(script: Script, pcName: String, sending: Boolean, onRun: () -> Unit, onDismiss: () -> Unit) {
    AgentSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.agents_script_confirm_title, script.name, pcName), style = MaterialTheme.typography.titleLarge)
            CommandLine(script.command, Modifier.fillMaxWidth())
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                G7RaisedButton(label = stringResource(R.string.action_cancel), onClick = onDismiss, enabled = !sending, modifier = Modifier.weight(1f))
                Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
                    G7PrimaryButton(label = stringResource(R.string.agents_script_run), onClick = onRun, enabled = !sending)
                    if (sending) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = MaterialTheme.colorScheme.onPrimary)
                }
            }
        }
    }
}

/** "dev is running" under the thread's status line, with Stop when this phone may stop it; a tap opens Scripts. */
@Composable
internal fun ScriptRunningLine(line: RunningLine, onOpen: () -> Unit, onStop: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 4.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onOpen)
            .heightIn(min = 44.dp)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
        Text(
            stringResource(R.string.agents_script_running_line, line.run.name),
            style = MaterialTheme.typography.labelLarge,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (line.canStop) G7RowAction(label = stringResource(R.string.agents_script_stop), onClick = onStop)
    }
}
