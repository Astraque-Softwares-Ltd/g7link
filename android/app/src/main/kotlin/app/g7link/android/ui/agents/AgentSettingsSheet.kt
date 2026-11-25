// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.core.AgentInfo

/**
 * One sheet for the model, its thinking (Claude) or reasoning (Codex) and the
 * mode (contract §Q6.3, the phone board 7B): the models the agent's CLI
 * reports by their exact names, the short names first and the rest under
 * "More models", "Other…" for a typed name; the chosen model's efforts; and
 * the phone's two modes. Nothing changes until Done, and on a thread it
 * applies to the next message.
 */
@Composable
internal fun AgentSettingsSheet(
    agentName: String,
    codex: Boolean,
    abilities: AgentAbilities,
    initial: ThreadSettings,
    /** A thread that exists: it says the change applies to the next message. */
    appliesLater: Boolean,
    onDone: (ThreadSettings) -> Unit,
    onDismiss: () -> Unit,
) {
    SettingsSheet(
        initialAgent = null,
        agents = null,
        fixed = FixedAgent(agentName, codex, abilities),
        initial = initial,
        appliesLater = appliesLater,
        onDone = { _, settings -> onDone(settings) },
        onDismiss = onDismiss,
    )
}

/**
 * The same sheet for a new thread ("Other agent, model or thinking…",
 * contract C §5.5): an Agent section on top, every agent of the PC with its
 * logo (one that can't start a thread is there, off, with why), and no Mode
 * (a new thread chooses it under "Asks before").
 */
@Composable
internal fun NewThreadSettingsSheet(
    agents: List<AgentInfo>,
    control: Boolean,
    initialAgent: String,
    initial: ThreadSettings,
    onDone: (agent: String, settings: ThreadSettings) -> Unit,
    onDismiss: () -> Unit,
) {
    SettingsSheet(
        initialAgent = initialAgent,
        agents = agents,
        fixed = null,
        initial = initial,
        appliesLater = false,
        onDone = { agent, settings -> if (agent != null) onDone(agent, settings) },
        onDismiss = onDismiss,
        control = control,
    )
}

/** The one agent a thread's sheet is for. */
private class FixedAgent(val name: String, val codex: Boolean, val abilities: AgentAbilities)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsSheet(
    initialAgent: String?,
    agents: List<AgentInfo>?,
    fixed: FixedAgent?,
    initial: ThreadSettings,
    appliesLater: Boolean,
    onDone: (String?, ThreadSettings) -> Unit,
    onDismiss: () -> Unit,
    control: Boolean = true,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scheme = MaterialTheme.colorScheme
    var agentId by rememberSaveable { mutableStateOf(initialAgent) }
    val agentInfo = agents?.firstOrNull { it.id == agentId }
    val abilities = fixed?.abilities ?: remember(agentInfo, control) { AgentControlFfi.abilities(agentInfo, control) }
    val agentName = fixed?.name ?: agentInfo?.name?.ifBlank { null } ?: agentId?.replaceFirstChar { it.uppercaseChar() }.orEmpty()
    val codex = fixed?.codex ?: (agentId == "codex")
    val models = abilities.models
    val sections = AgentSettings.sections(models)
    val sameNamed = remember(models) { AgentSettings.sameNamed(models) }
    var model by rememberSaveable { mutableStateOf(initial.model) }
    var effort by rememberSaveable { mutableStateOf(initial.effort) }
    var mode by rememberSaveable { mutableStateOf(initial.mode) }
    var custom by rememberSaveable { mutableStateOf(initial.customModel || (initial.model != null && models.none { it.id == initial.model })) }
    var typed by rememberSaveable { mutableStateOf(if (custom) initial.model.orEmpty() else "") }
    var more by rememberSaveable { mutableStateOf(sections.more.any { it.id == initial.model }) }
    val settings = ThreadSettings(model = model, effort = effort, mode = mode, customModel = custom)
    val typedOk = !custom || AgentSettings.isModelId(typed.trim())
    val pick: (String?) -> Unit = { id ->
        val next = AgentSettings.pickModel(models, settings, id)
        custom = false
        model = next.model
        effort = next.effort
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = scheme.surfaceContainerHigh) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp)
                .padding(bottom = 20.dp),
        ) {
            Text(
                stringResource(R.string.agents_settings_title, agentName),
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
            )
            if (agents != null) {
                G7SectionLabel(stringResource(R.string.agents_pick_agent))
                AgentChoiceRows(
                    agents = agents,
                    selected = agentId,
                    onSelect = { id ->
                        if (id != agentId) {
                            // Another agent starts on its own default; back to the first, on what it had.
                            val back = id == initialAgent
                            agentId = id
                            custom = back && initial.customModel
                            model = if (back) initial.model else null
                            effort = if (back) initial.effort else null
                            typed = if (back && initial.customModel) initial.model.orEmpty() else ""
                            more = false
                        }
                    },
                )
            }
            G7SectionLabel(stringResource(R.string.agents_settings_model))
            val defaultName = AgentSettings.defaultModel(models)?.let(AgentSettings::shownName)
            ChoiceRow(
                label = defaultName ?: stringResource(R.string.agents_agent_default, agentName),
                tag = if (defaultName != null) stringResource(R.string.agents_model_default) else null,
                selected = model == null && !custom,
                onClick = { pick(null) },
            )
            for (option in sections.first) ModelRow(option, note = null, selected = !custom && model == option.id) { pick(option.id) }
            if (sections.more.isNotEmpty()) {
                ChoiceRow(
                    label = stringResource(R.string.agents_model_more),
                    tag = sections.more.size.toString(),
                    role = Role.Button,
                    onClick = { more = !more },
                ) {
                    Icon(if (more) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                }
                if (more) {
                    for (option in sections.more) {
                        ModelRow(option, note = option.id.takeIf { it in sameNamed }, selected = !custom && model == option.id) { pick(option.id) }
                    }
                }
            }
            ChoiceRow(
                label = stringResource(R.string.agents_model_other),
                selected = custom,
                onClick = {
                    custom = true
                    effort = null
                },
            )
            if (custom) {
                AgentTextField(
                    value = typed,
                    onValueChange = { typed = it.take(MAX_TYPED) },
                    hint = stringResource(R.string.agents_model_hint, agentName),
                    background = scheme.surfaceContainer,
                    maxLines = 1,
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                )
                if (typed.isNotBlank() && !typedOk) {
                    Text(
                        stringResource(R.string.agents_model_invalid),
                        style = MaterialTheme.typography.bodySmall,
                        color = scheme.error,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(horizontal = 12.dp),
                    )
                }
            }
            val efforts = if (custom) emptyList() else AgentSettings.efforts(models, settings)
            if (efforts.isNotEmpty()) {
                G7SectionLabel(effortWord(codex))
                val chosenModel = AgentSettings.model(models, model)
                // "Auto" leaves it to the model when the CLI names no default level (Claude's); Codex's own default shows chosen instead.
                val auto = chosenModel?.defaultEffort == null
                val shown = AgentSettings.effectiveEffort(models, settings)
                Row(
                    modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 4.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    if (auto) {
                        G7RaisedButton(
                            label = stringResource(R.string.agents_effort_auto),
                            selected = shown == null,
                            onClick = { effort = null },
                        )
                    }
                    for (level in efforts) {
                        G7RaisedButton(
                            label = AgentSettings.effortLabel(level),
                            selected = level == shown,
                            onClick = { effort = AgentSettings.pickEffort(models, settings, level).effort },
                        )
                    }
                }
            }
            if (agents == null) {
                G7SectionLabel(stringResource(R.string.agents_settings_mode))
                for (each in AgentSettings.phoneModes(abilities)) {
                    ChoiceRow(
                        label = modeLabel(each),
                        lines = if (each == ThreadMode.SUPERVISED) listOf(SubLine(stringResource(R.string.agents_mode_default_note))) else emptyList(),
                        selected = mode == each,
                        onClick = { mode = AgentSettings.pickMode(abilities, settings, each).mode },
                    )
                }
                if (codex && !abilities.can(AgentAbilities.PLAN)) {
                    Text(
                        stringResource(R.string.agents_plan_unavailable),
                        style = MaterialTheme.typography.bodySmall,
                        color = scheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                    )
                }
            }
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    if (appliesLater) stringResource(R.string.agents_model_applies) else "",
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
                G7PrimaryButton(
                    label = stringResource(R.string.agents_settings_done),
                    enabled = typedOk && (!custom || typed.isNotBlank()) && (agents == null || agentInfo?.let { it.installed && it.signedIn } == true),
                    onClick = {
                        val chosen = if (custom) AgentSettings.typeModel(settings, typed) else settings
                        if (chosen != null) onDone(agentId, chosen)
                    },
                    modifier = Modifier.weight(0.6f),
                )
            }
        }
    }
}

@Composable
private fun ModelRow(option: ModelOption, note: String?, selected: Boolean, onClick: () -> Unit) {
    val name = AgentSettings.shownName(option) ?: option.id
    ChoiceRow(
        label = name,
        tag = if (option.isDefault) stringResource(R.string.agents_model_default) else null,
        lines = listOfNotNull((note ?: option.note)?.takeIf { it.isNotBlank() }?.let { SubLine(it) }),
        selected = selected,
        onClick = onClick,
    )
}

/** What an agent's effort is called: Claude's is thinking, Codex's reasoning. */
@Composable
internal fun effortWord(codex: Boolean): String =
    stringResource(if (codex) R.string.agents_reasoning else R.string.agents_thinking)

/** A mode in words (contract §Q8). */
@Composable
internal fun modeLabel(mode: ThreadMode): String = when (mode) {
    ThreadMode.SUPERVISED -> stringResource(R.string.agents_mode_supervised)
    ThreadMode.AUTO_EDIT -> stringResource(R.string.agents_mode_auto_edit)
    ThreadMode.PLAN -> stringResource(R.string.agents_mode_plan)
}

/** A typed model name never needs more than the wire's 64 bytes; a little room to see it's too long. */
private const val MAX_TYPED = 80
