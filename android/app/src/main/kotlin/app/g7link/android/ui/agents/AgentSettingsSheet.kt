// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7SectionLabel

/**
 * One sheet for the model, its effort and the mode (contract §Q6.3, the phone
 * board 7B): the models the agent's CLI reports, the short names first and the
 * rest under "More models", "Other…" for a typed name; the chosen model's
 * efforts; and the phone's two modes. Nothing changes until Done, and on a
 * thread it applies to the next message.
 */
@OptIn(ExperimentalMaterial3Api::class)
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
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scheme = MaterialTheme.colorScheme
    val models = abilities.models
    val sections = AgentSettings.sections(models)
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
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
            )
            G7SectionLabel(stringResource(R.string.agents_settings_model))
            val defaultName = AgentSettings.defaultModel(models)?.takeIf { it.id != "default" }?.name
            G7Row(
                label = stringResource(R.string.agents_model_default),
                sublabel = defaultName,
                role = Role.RadioButton,
                active = model == null && !custom,
                onClick = { pick(null) },
            )
            for (option in sections.first) ModelRow(option, selected = !custom && model == option.id) { pick(option.id) }
            if (sections.more.isNotEmpty()) {
                G7Row(
                    label = stringResource(R.string.agents_model_more),
                    note = sections.more.size.toString(),
                    onClick = { more = !more },
                ) {
                    Icon(if (more) G7Icons.chevronDown else G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                }
                if (more) {
                    for (option in sections.more) ModelRow(option, selected = !custom && model == option.id) { pick(option.id) }
                }
            }
            G7Row(
                label = stringResource(R.string.agents_model_other),
                role = Role.RadioButton,
                active = custom,
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
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                )
                if (typed.isNotBlank() && !typedOk) {
                    Text(
                        stringResource(R.string.agents_model_invalid),
                        style = MaterialTheme.typography.bodySmall,
                        color = scheme.error,
                        modifier = Modifier.padding(horizontal = 12.dp),
                    )
                }
            }
            val efforts = if (custom) emptyList() else AgentSettings.efforts(models, settings)
            if (efforts.isNotEmpty()) {
                G7SectionLabel(stringResource(R.string.agents_settings_effort))
                val shown = AgentSettings.effectiveEffort(models, settings)
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    for (level in efforts) {
                        G7RaisedButton(
                            label = AgentSettings.effortLabel(level),
                            selected = level == shown,
                            onClick = { effort = AgentSettings.pickEffort(models, settings, level).effort },
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            }
            G7SectionLabel(stringResource(R.string.agents_settings_mode))
            for (each in AgentSettings.phoneModes(abilities)) {
                G7Row(
                    label = modeLabel(each),
                    sublabel = if (each == ThreadMode.SUPERVISED) stringResource(R.string.agents_mode_default_note) else null,
                    role = Role.RadioButton,
                    active = mode == each,
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
                    enabled = typedOk && (!custom || typed.isNotBlank()),
                    onClick = {
                        val chosen = if (custom) AgentSettings.typeModel(settings, typed) else settings
                        if (chosen != null) onDone(chosen)
                    },
                    modifier = Modifier.weight(0.6f),
                )
            }
        }
    }
}

@Composable
private fun ModelRow(option: ModelOption, selected: Boolean, onClick: () -> Unit) {
    G7Row(
        label = option.name.ifBlank { option.id },
        sublabel = option.note,
        role = Role.RadioButton,
        active = selected,
        onClick = onClick,
    )
}

/** A mode in words (contract §Q8). */
@Composable
internal fun modeLabel(mode: ThreadMode): String = when (mode) {
    ThreadMode.SUPERVISED -> stringResource(R.string.agents_mode_supervised)
    ThreadMode.AUTO_EDIT -> stringResource(R.string.agents_mode_auto_edit)
    ThreadMode.PLAN -> stringResource(R.string.agents_mode_plan)
}

/** A typed model name never needs more than the wire's 64 bytes; a little room to see it's too long. */
private const val MAX_TYPED = 80
