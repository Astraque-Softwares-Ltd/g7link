// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.home

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.style.Hyphens
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.g7link.android.G7LinkApplication
import app.g7link.android.R
import app.g7link.android.core.PcCaptureFormat
import app.g7link.android.core.PcCaptureState
import app.g7link.android.ui.capture.rememberElapsed
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.g7Plate
import app.g7link.android.ui.theme.g7StateFraction
import app.g7link.android.ui.theme.g7Tints
import app.g7link.core.DeckState

/** Tiles in a row on Home. */
private const val TILE_COLUMNS = 4

/** How far a disabled tile fades, as Home's rows do. */
private const val DISABLED_ALPHA = 0.45f

private val TILE_GAP = 8.dp
private val TILE_HEIGHT = 68.dp
private val TILE_SHAPE = RoundedCornerShape(10.dp)

/**
 * One of Home's tiles: its glyph in its tint over a short label. [active] is a mode or session
 * that's on (drawn in the accent); [toggle] makes the tile a switch for it; [busy] is an action
 * under way (a spinner, and no second tap). [id] keeps the tile's state while others come and go.
 */
internal class HomeTileSpec(
    val id: String,
    val label: String,
    val icon: ImageVector,
    val tint: Color,
    val enabled: Boolean = true,
    val active: Boolean = false,
    val busy: Boolean = false,
    val highlighted: Boolean = false,
    val toggle: Boolean = false,
    val onClick: () -> Unit,
)

/** [tiles] four to a row; a short last row keeps its tiles the same width as the others. */
@Composable
internal fun HomeTileGrid(tiles: List<HomeTileSpec>, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier.fillMaxWidth().padding(horizontal = 2.dp),
        verticalArrangement = Arrangement.spacedBy(TILE_GAP),
    ) {
        tiles.chunked(TILE_COLUMNS).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(TILE_GAP)) {
                row.forEach { tile ->
                    key(tile.id) { HomeTile(tile, Modifier.weight(1f)) }
                }
                repeat(TILE_COLUMNS - row.size) { Spacer(Modifier.weight(1f)) }
            }
        }
    }
}

@Composable
private fun HomeTile(tile: HomeTileSpec, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val activeShown = g7StateFraction(tile.active)
    val highlightShown = g7StateFraction(tile.highlighted && !tile.active)
    val enabledShown = g7StateFraction(tile.enabled)
    val labelIdle = scheme.onSurface
    val labelActive = scheme.primary
    Column(
        modifier = modifier
            .heightIn(min = TILE_HEIGHT)
            .clip(TILE_SHAPE)
            .g7Plate(highlightShown, scheme.surfaceContainerHigh, edge = null, TILE_SHAPE, under = scheme.surfaceContainer)
            .g7Plate(activeShown, scheme.primary.copy(alpha = 0.18f), scheme.primary, TILE_SHAPE)
            .clickable(
                enabled = tile.enabled && !tile.busy,
                role = if (tile.toggle) Role.Switch else Role.Button,
                onClick = tile.onClick,
            )
            .semantics {
                if (tile.toggle) {
                    toggleableState = ToggleableState(tile.active)
                } else if (tile.active) {
                    selected = true
                }
            }
            .padding(horizontal = 4.dp, vertical = 8.dp)
            .graphicsLayer { alpha = DISABLED_ALPHA + (1f - DISABLED_ALPHA) * enabledShown.value },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterVertically),
    ) {
        Box(Modifier.size(22.dp), contentAlignment = Alignment.Center) {
            if (tile.busy) {
                CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp, color = scheme.primary)
            } else {
                Icon(tile.icon, contentDescription = null, tint = tile.tint, modifier = Modifier.size(22.dp))
            }
        }
        BasicText(
            tile.label,
            // A long word ("Zwischenablage") breaks with a hyphen, not anywhere.
            style = MaterialTheme.typography.labelMedium.copy(textAlign = TextAlign.Center, hyphens = Hyphens.Auto),
            color = { lerp(labelIdle, labelActive, activeShown.value) },
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A tile two to a row: its glyph, its label and a count, for Activity. */
@Composable
internal fun HomeWideTile(
    label: String,
    icon: ImageVector,
    tint: Color,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    note: String? = null,
    highlighted: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    val highlightShown = g7StateFraction(highlighted)
    Row(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(TILE_SHAPE)
            .g7Plate(highlightShown, scheme.surfaceContainerHigh, edge = null, TILE_SHAPE, under = scheme.surfaceContainer)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(20.dp))
        Text(
            label,
            style = MaterialTheme.typography.labelLarge,
            color = scheme.onSurface,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (!note.isNullOrEmpty()) {
            Text(note, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 1)
        }
    }
}

/**
 * The PC's sound on Home, as one card: the volume (the glyph mutes), the output it plays on, and
 * two switches drawn as buttons: this phone as the PC's speaker and, where the PC offers it, the
 * phone's own audio on the PC over Bluetooth.
 */
@Composable
internal fun HomeSoundCard(
    pcId: String,
    pcName: String,
    deckState: DeckState,
    onSetPcAudio: (volume: Int?, muted: Boolean?) -> Unit,
    modifier: Modifier = Modifier,
    highlighted: Boolean = false,
    btMedia: Boolean? = null,
    onSetBtMedia: (on: Boolean) -> Unit = {},
) {
    val context = LocalContext.current
    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    val outputs = rememberPcOutputs(deckState)
    var showOutputs by remember { mutableStateOf(false) }
    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 2.dp)
            .clip(TILE_SHAPE)
            .background(if (highlighted) scheme.surfaceContainerHigh else scheme.surfaceContainer)
            .padding(bottom = 12.dp),
    ) {
        PcVolumeRow(deckState = deckState, onSetPcAudio = onSetPcAudio)
        val output = outputs.default
        if (output != null) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp)
                    .clickable(role = Role.Button) { showOutputs = true }
                    // Under the slider, past the volume glyph.
                    .padding(start = 48.dp, end = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    stringResource(R.string.pc_audio_output, output.name),
                    style = MaterialTheme.typography.bodySmall,
                    color = scheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                Icon(G7Icons.chevronDown, contentDescription = null, tint = scheme.onSurfaceVariant, modifier = Modifier.size(14.dp))
            }
        }
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = if (output == null) 4.dp else 0.dp),
            horizontalArrangement = Arrangement.spacedBy(TILE_GAP),
        ) {
            HomeToggleButton(
                label = stringResource(R.string.home_tile_phone_speaker),
                icon = G7Icons.speaker,
                tint = tints.teal,
                checked = outputs.streamingToPhone,
                onCheckedChange = { on -> onSetPcAudio(if (on) 201 else 200, null) },
                modifier = Modifier.weight(1f),
            )
            if (btMedia != null) {
                HomeToggleButton(
                    label = stringResource(R.string.home_tile_phone_audio, pcName),
                    icon = G7Icons.bluetooth,
                    tint = tints.blue,
                    checked = btMedia,
                    onCheckedChange = onSetBtMedia,
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }

    if (showOutputs && outputs.hardware.isNotEmpty()) {
        PcAudioOutputsDialog(
            pcName = pcName,
            devices = outputs.hardware,
            onPick = { picked ->
                (context.applicationContext as? G7LinkApplication)?.core?.setPcAudioOutput(pcId, picked)
                showOutputs = false
            },
            onDismiss = { showOutputs = false },
        )
    }
}

/** A switch as a raised button: on is the accent, as Home's active tiles are. */
@Composable
private fun HomeToggleButton(
    label: String,
    icon: ImageVector,
    tint: Color,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    val scheme = MaterialTheme.colorScheme
    val checkedShown = g7StateFraction(checked)
    val labelIdle = scheme.onSurface
    val labelChecked = scheme.primary
    Row(
        modifier = modifier
            .heightIn(min = 48.dp)
            .clip(TILE_SHAPE)
            .g7Plate(checkedShown, scheme.primary.copy(alpha = 0.18f), scheme.primary, TILE_SHAPE, under = scheme.surfaceContainerHigh)
            .toggleable(value = checked, role = Role.Switch, onValueChange = onCheckedChange)
            .padding(horizontal = 8.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(18.dp))
        BasicText(
            label,
            style = MaterialTheme.typography.labelLarge,
            color = { lerp(labelIdle, labelChecked, checkedShown.value) },
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
    }
}

/** "Record", "Stop · 0:42", or "Approve on your PC" while the PC asks: Record screen, short for a tile. */
@Composable
internal fun recordTileLabel(capture: PcCaptureState): String {
    val elapsed = rememberElapsed(capture.recordingSince)
    return when {
        capture.recordingBusy && capture.approve -> stringResource(R.string.pc_capture_approve)
        capture.recordingSince != null -> stringResource(R.string.home_tile_stop_recording, PcCaptureFormat.elapsed(elapsed))
        else -> stringResource(R.string.home_tile_record)
    }
}
