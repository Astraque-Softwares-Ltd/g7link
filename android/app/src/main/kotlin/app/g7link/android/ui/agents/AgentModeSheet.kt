// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.g7Tints

// An agent's modes in words and pictures (plans/modes-approvals-composer.md §4.2):
// each agent's own names and lines, G7's short word and icon for the reply
// box's chip, and the list the chip, a new thread and Implement open.

/** A mode's name as its agent says it: Claude's "Manual", Codex's "Ask for approval". */
@Composable
internal fun modeName(mode: ThreadMode, codex: Boolean): String = stringResource(
    when (mode) {
        ThreadMode.ASK -> if (codex) R.string.agents_mode_codex_ask else R.string.agents_mode_claude_ask
        ThreadMode.ACCEPT_EDITS -> R.string.agents_mode_claude_edits
        ThreadMode.AUTO -> if (codex) R.string.agents_mode_codex_auto else R.string.agents_mode_claude_auto
        ThreadMode.PLAN -> R.string.agents_plan
        ThreadMode.FULL -> if (codex) R.string.agents_mode_codex_full else R.string.agents_mode_claude_full
    },
)

/** What a mode does, in the line its agent shows under the name. */
@Composable
internal fun modeLine(mode: ThreadMode, codex: Boolean): String = stringResource(
    when (mode) {
        ThreadMode.ASK -> if (codex) R.string.agents_mode_codex_ask_line else R.string.agents_mode_claude_ask_line
        ThreadMode.ACCEPT_EDITS -> R.string.agents_mode_claude_edits_line
        ThreadMode.AUTO -> if (codex) R.string.agents_mode_codex_auto_line else R.string.agents_mode_claude_auto_line
        ThreadMode.PLAN -> if (codex) R.string.agents_mode_codex_plan_line else R.string.agents_mode_claude_plan_line
        ThreadMode.FULL -> if (codex) R.string.agents_mode_codex_full_line else R.string.agents_mode_claude_full_line
    },
)

/** The reply box chip's one word, the same for every agent: Ask, Edits, Auto, Plan, Full. */
@Composable
internal fun modeShort(mode: ThreadMode): String = stringResource(
    when (mode) {
        ThreadMode.ASK -> R.string.agents_mode_short_ask
        ThreadMode.ACCEPT_EDITS -> R.string.agents_mode_short_edits
        ThreadMode.AUTO -> R.string.agents_mode_short_auto
        ThreadMode.PLAN -> R.string.agents_plan
        ThreadMode.FULL -> R.string.agents_mode_short_full
    },
)

internal fun modeIcon(mode: ThreadMode): ImageVector = when (mode) {
    ThreadMode.ASK -> G7Icons.hand
    ThreadMode.ACCEPT_EDITS -> G7Icons.filePen
    ThreadMode.AUTO -> G7Icons.sparkles
    ThreadMode.PLAN -> G7Icons.listChecks
    ThreadMode.FULL -> G7Icons.shieldAlert
}

/** A mode's tint: amber for the one that asks nothing, else [otherwise]. */
@Composable
internal fun modeTint(mode: ThreadMode, otherwise: Color): Color = if (AgentModes.warns(mode)) g7Tints().amber else otherwise

/** One row per mode: its icon, its agent's name for it and the line under it, the [selected] one marked. */
@Composable
internal fun ModeRows(modes: List<ThreadMode>, codex: Boolean, selected: ThreadMode?, onPick: (ThreadMode) -> Unit) {
    for (each in modes) {
        ChoiceRow(
            label = modeName(each, codex),
            icon = modeIcon(each),
            iconTint = modeTint(each, Color.Unspecified),
            lines = listOf(SubLine(modeLine(each, codex), maxLines = 2)),
            selected = each == selected,
            onClick = { onPick(each) },
        )
    }
}

/**
 * The mode list on its own: from the reply box's chip (a thread's mode, for its
 * next message), a new thread's Mode, or Implement (the modes a plan may run
 * in). A pick is the answer; [note] goes under the list.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AgentModeSheet(
    title: String,
    modes: List<ThreadMode>,
    codex: Boolean,
    selected: ThreadMode?,
    onPick: (ThreadMode) -> Unit,
    onDismiss: () -> Unit,
    note: String? = null,
) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scheme = MaterialTheme.colorScheme
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = scheme.surfaceContainerHigh) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp)
                .padding(bottom = 20.dp),
        ) {
            Text(
                title,
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
            )
            ModeRows(modes, codex, selected, onPick)
            if (note != null) {
                Text(
                    note,
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
                )
            }
        }
    }
}
