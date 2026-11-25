// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.g7Plate
import app.g7link.android.ui.theme.g7StateFraction
import app.g7link.android.ui.theme.g7Tints

/**
 * Browse (contract C §5.7, docs/protocol/agents.md §15): the PC's Home and
 * disks as chips, the way down as a breadcrumb, and the folders where it is,
 * repositories with the branch glyph and a direct "Use", the PC's own agent
 * folders marked. "Use this folder" takes the one shown. A folder already
 * among the agent folders is chosen at once; a new one is added, and while
 * the PC asks its user, "Approve on <PC>" says so.
 */
@Composable
internal fun AgentPlacesPage(
    pcName: String,
    state: PlacesState,
    /** The PC asks its user before a new folder is added from this phone. */
    asksFirst: Boolean,
    onOpen: (place: String, folder: String?) -> Unit,
    onMore: () -> Unit,
    onRetry: () -> Unit,
    onUse: (place: String, folder: String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    val roots = state.roots.orEmpty()
    val adding = state.adding
    Column(modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OneLine(stringResource(R.string.agents_where_all, pcName), MaterialTheme.typography.titleLarge, scheme.onSurface, Modifier.padding(top = 4.dp))
            if (roots.isNotEmpty()) RootChips(roots, current = AgentPlacesModel.crumbs(state.place).firstOrNull()?.second, onOpen = { onOpen(it, null) })
            if (state.place.isNotEmpty()) Crumbs(state.place, roots, onOpen = { onOpen(it, null) })
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp)) {
            if (state.place.isEmpty()) {
                itemsIndexed(state.entries, key = { _, root -> "root/" + root.name }) { index, root ->
                    ChoiceRow(
                        label = rootName(root.name, index),
                        icon = rootIcon(root.name, index),
                        role = Role.Button,
                        onClick = { onOpen(root.name, null) },
                    ) {
                        Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                    }
                }
            } else {
                items(state.entries, key = { "dir/" + it.name }) { entry ->
                    val place = AgentPlacesModel.child(state.place, entry.name)
                    ChoiceRow(
                        label = entry.name,
                        icon = if (entry.repo) AgentGlyphs.branch else G7Icons.folder,
                        iconTint = if (entry.repo) scheme.onSurfaceVariant else g7Tints().amber,
                        lines = if (entry.folder != null) listOf(SubLine(stringResource(R.string.agents_places_in_folders))) else emptyList(),
                        role = Role.Button,
                        onClick = { onOpen(place, entry.folder) },
                    ) {
                        if (entry.repo || entry.folder != null) {
                            G7RowAction(
                                label = stringResource(R.string.agents_places_use_short),
                                onClick = { onUse(place, entry.folder) },
                                enabled = adding == null,
                            )
                        }
                    }
                }
            }
            val failure = state.failure
            when {
                state.loading -> item(key = "loading") {
                    Box(Modifier.fillMaxWidth().heightIn(min = 56.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                    }
                }
                failure != null -> item(key = "failed") {
                    Column(Modifier.fillMaxWidth().padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Text(placesFailureText(failure, pcName, adding = false), style = MaterialTheme.typography.bodyMedium, color = scheme.error)
                        G7RaisedButton(label = stringResource(R.string.agents_try_again), onClick = onRetry, modifier = Modifier.padding(horizontal = 24.dp))
                    }
                }
                state.more -> item(key = "more") {
                    ChoiceRow(label = stringResource(R.string.agents_places_show_more), role = Role.Button, onClick = onMore) {
                        Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(12.dp))
                    }
                }
                state.entries.isEmpty() -> item(key = "empty") {
                    Text(
                        stringResource(R.string.agents_places_empty),
                        style = MaterialTheme.typography.bodyMedium,
                        color = scheme.onSurfaceVariant,
                        modifier = Modifier.padding(12.dp),
                    )
                }
            }
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (adding != null && adding.prompted) ApproveCard(pcName, adding.place)
            state.addFailure?.let { Text(placesFailureText(it, pcName, adding = true), style = MaterialTheme.typography.bodyMedium, color = scheme.error) }
            val canAdd = AgentPlacesModel.canAdd(state.place)
            if (asksFirst && canAdd && adding == null && state.folder == null) {
                Text(stringResource(R.string.agents_where_note, pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
            }
            if (adding != null && !adding.prompted) {
                Box(Modifier.fillMaxWidth().heightIn(min = 48.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                }
            } else if (adding == null) {
                G7PrimaryButton(
                    label = stringResource(R.string.agents_places_use),
                    enabled = canAdd,
                    onClick = { onUse(state.place, state.folder) },
                )
            }
        }
    }
}

/** The PC's roots as chips: Home, then its disks. */
@Composable
private fun RootChips(roots: List<PlaceEntry>, current: String?, onOpen: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        roots.forEachIndexed { index, root ->
            val on = root.name == current
            val shape = RoundedCornerShape(8.dp)
            val shown = g7StateFraction(on)
            Row(
                modifier = Modifier
                    .heightIn(min = 40.dp)
                    .clip(shape)
                    .border(1.dp, scheme.outlineVariant, shape)
                    .g7Plate(shown, scheme.primary.copy(alpha = 0.14f), scheme.primary, shape)
                    .clickable(role = Role.Tab) { onOpen(root.name) }
                    .semantics { selected = on }
                    .padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Icon(rootIcon(root.name, index), contentDescription = null, tint = if (on) scheme.primary else scheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
                OneLine(rootName(root.name, index), MaterialTheme.typography.labelLarge, if (on) scheme.primary else scheme.onSurface)
            }
        }
    }
}

/** Where the page is, a step per folder from its root; each step opens that folder. Scrolled to the end, on one line. */
@Composable
private fun Crumbs(place: String, roots: List<PlaceEntry>, onOpen: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val scroll = rememberScrollState()
    LaunchedEffect(place, scroll.maxValue) { scroll.scrollTo(scroll.maxValue) }
    val crumbs = AgentPlacesModel.crumbs(place)
    Row(Modifier.fillMaxWidth().horizontalScroll(scroll), verticalAlignment = Alignment.CenterVertically) {
        crumbs.forEachIndexed { index, (name, at) ->
            val last = index == crumbs.lastIndex
            val shownName = if (index == 0) rootName(name, if (roots.isEmpty()) 0 else roots.indexOfFirst { it.name == name }) else name
            Text(
                shownName,
                style = MaterialTheme.typography.labelLarge,
                color = if (last) scheme.onSurface else scheme.primary,
                maxLines = 1,
                modifier = Modifier
                    .heightIn(min = 40.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .clickable(enabled = !last, role = Role.Button) { onOpen(at) }
                    .padding(horizontal = 6.dp, vertical = 10.dp),
            )
            if (!last) Icon(G7Icons.chevronRight, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(10.dp))
        }
    }
}

/** While the PC asks its user about the folder: "Approve on <PC>", with the folder's name. */
@Composable
private fun ApproveCard(pcName: String, place: String) {
    val scheme = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .border(1.dp, scheme.primary, shape)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(AgentGlyphs.laptop, contentDescription = null, tint = scheme.primary, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            OneLine(stringResource(R.string.agents_places_approve, pcName), MaterialTheme.typography.titleSmall, scheme.onSurface)
            OneLine(place.substringAfterLast('/'), MaterialTheme.typography.bodySmall, scheme.onSurfaceVariant)
        }
        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
    }
}

/** Why a listing or an add didn't work, in words. */
@Composable
private fun placesFailureText(failure: PlacesFailure, pcName: String, adding: Boolean): String = when (failure) {
    PlacesFailure.DENIED ->
        if (adding) stringResource(R.string.agents_places_add_denied, pcName) else stringResource(R.string.agents_access_title, pcName)
    PlacesFailure.NOT_FOUND -> stringResource(R.string.agents_places_gone)
    PlacesFailure.BUSY -> stringResource(R.string.agents_places_busy, pcName)
    PlacesFailure.LIMIT -> stringResource(R.string.agents_places_limit, pcName)
    PlacesFailure.UNSUPPORTED -> stringResource(R.string.agents_unavailable, pcName)
    PlacesFailure.TIMEOUT, PlacesFailure.OFFLINE -> stringResource(R.string.agents_unreachable_title, pcName)
    PlacesFailure.OTHER -> stringResource(R.string.agents_failed_generic)
}
