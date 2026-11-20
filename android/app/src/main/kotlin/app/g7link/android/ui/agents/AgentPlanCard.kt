// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.background
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
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.AgentBlock
import app.g7link.core.AgentBlockKind
import app.g7link.core.AgentPlanState
import app.g7link.core.AgentRow
import app.g7link.core.AgentRowKind

/**
 * What the thread's list shows, one entry each: a row, or a plan with its
 * blocks, which the FFI sends as a `Plan` row followed by the plan's `Block`
 * rows (contract §Q2.3) and the phone draws as one card.
 */
@Immutable
internal sealed interface RowUnit {
    val key: String

    data class One(val row: AgentRow) : RowUnit {
        override val key: String get() = row.key
    }

    data class Plan(val head: AgentRow, val kind: AgentRowKind.Plan, val blocks: List<AgentRow>) : RowUnit {
        override val key: String get() = head.key
    }
}

/** Which rows are a plan card's: its `Plan` row and the `Block` rows of its item that follow it. */
internal object AgentPlanCards {
    /** The rows in order, a plan's blocks gathered under it. */
    fun units(rows: List<AgentRow>): List<RowUnit> {
        val units = ArrayList<RowUnit>(rows.size)
        var index = 0
        while (index < rows.size) {
            val row = rows[index]
            val kind = row.kind
            if (kind is AgentRowKind.Plan) {
                val blocks = ArrayList<AgentRow>()
                var next = index + 1
                while (next < rows.size && rows[next].kind is AgentRowKind.Block && rows[next].itemId == row.itemId) {
                    blocks += rows[next]
                    next++
                }
                units += RowUnit.Plan(row, kind, blocks)
                index = next
            } else {
                units += RowUnit.One(row)
                index++
            }
        }
        return units
    }

    /** The plan's first block when it's a heading: the card's title, not repeated in its body. */
    fun title(blocks: List<AgentBlock>): String? =
        blocks.firstOrNull()?.takeIf { it.kind is AgentBlockKind.Heading }?.text?.takeIf { it.isNotBlank() }
}

/**
 * A plan as a card (the phone board 7E): "Plan" and its title, Copy, the blocks
 * the FFI shows (a long plan folded behind "Show the whole plan"), then "Change
 * the plan" and "Implement". Both wait while the thread runs and are only on
 * its newest proposed plan; a replaced plan shows its header until opened.
 */
@Composable
internal fun PlanCard(unit: RowUnit.Plan, context: RowsContext, actions: RowActions) {
    val scheme = MaterialTheme.colorScheme
    val tints = g7Tints()
    val itemId = unit.head.itemId
    val blocks = unit.blocks
    val title = AgentPlanCards.title(blocks.mapNotNull { (it.kind as? AgentRowKind.Block)?.block })
    val body = if (title != null) blocks.drop(1) else blocks
    val state = unit.kind.state
    val replaced = state == AgentPlanState.REPLACED
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(scheme.surfaceContainer)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 36.dp)
                .then(if (replaced) Modifier.clip(RoundedCornerShape(6.dp)).clickable(role = Role.Button) { actions.toggle(unit.key) } else Modifier),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(G7Icons.clipboardList, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
            Text(stringResource(R.string.agents_plan), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant, maxLines = 1)
            Text(
                title.orEmpty(),
                style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            val text = context.items[itemId]?.text
            if (!replaced && !text.isNullOrBlank()) CopyButton { actions.copy(text) }
        }
        when (state) {
            AgentPlanState.REPLACED -> Text(stringResource(R.string.agents_plan_replaced), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            AgentPlanState.IMPLEMENTED -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(G7Icons.check, contentDescription = null, tint = tints.teal, modifier = Modifier.size(14.dp))
                Text(stringResource(R.string.agents_plan_implemented), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            AgentPlanState.PROPOSED -> Unit
        }
        for (row in body) {
            val kind = row.kind as? AgentRowKind.Block ?: continue
            BlockRow(row, kind, context.items[row.itemId], actions)
        }
        // The FFI folds a long plan: the rest comes once its key is open.
        if (!replaced && !unit.kind.open && unit.kind.more > 0u) {
            Row(
                modifier = Modifier
                    .heightIn(min = 36.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .clickable(role = Role.Button) { actions.toggle(unit.key) }
                    .padding(end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                Text(stringResource(R.string.agents_plan_show_whole), style = MaterialTheme.typography.labelLarge, color = scheme.onSurfaceVariant)
            }
        }
        if (state == AgentPlanState.PROPOSED) {
            val newest = context.newestPlan == itemId
            val enabled = newest && !context.running && !context.implementing
            ButtonPair(
                secondary = stringResource(R.string.agents_plan_change),
                onSecondary = actions.changePlan,
                secondaryEnabled = enabled,
                primary = stringResource(R.string.agents_plan_implement),
                onPrimary = { actions.implement(itemId) },
                primaryEnabled = enabled,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
    }
}
