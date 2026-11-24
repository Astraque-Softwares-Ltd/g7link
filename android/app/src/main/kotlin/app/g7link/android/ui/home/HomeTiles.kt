// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.home

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
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

/** Tiles in a row on Home, where their names fit. */
private const val TILE_COLUMNS = 4

/** How far a disabled tile fades, as Home's rows do. */
private const val DISABLED_ALPHA = 0.45f

private val TILE_GAP = 8.dp
private val TILE_HEIGHT = 68.dp
private val TILE_SHAPE = RoundedCornerShape(10.dp)

/** Space each side of a tile's name. */
private val TILE_LABEL_INSET = 4.dp

/**
 * Every tile's name on Home, whether or not its PC offers it now: each grid measures them all,
 * so the grids keep one tile size and don't change it as the PC connects or a state changes.
 */
private val HOME_TILE_NAMES = listOf(
    R.string.home_files,
    R.string.home_folder,
    R.string.home_tile_clipboard,
    R.string.home_link,
    R.string.home_tile_trackpad,
    R.string.action_deck_pc,
    R.string.home_tile_pc_files,
    R.string.pc_capture_screenshot,
    R.string.home_tile_record,
    R.string.action_ring_device,
    R.string.home_lock,
    R.string.action_sleep_pc,
    R.string.action_webcam_pc,
    R.string.home_voice_note,
    R.string.home_desk_mode,
    R.string.home_tile_microphone,
)

/**
 * One of Home's tiles: its glyph in its tint over its name, on one line. [active] is a mode or
 * session that's on (drawn in the accent); [toggle] makes the tile a switch for it; [busy] is an
 * action under way (a spinner, and no second tap). The name stays while its state changes; where
 * the tile shows something else, such as a recording's time, [description] is what a screen
 * reader says instead. [id] keeps the tile's state while others come and go.
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
    val description: String? = null,
    val onClick: () -> Unit,
)

/**
 * [tiles] four to a row, or fewer where a tile's name on Home wouldn't fit on one line in a
 * quarter of the width (a long translation, larger text, a narrow phone). A short last row keeps
 * its tiles the same width as the others.
 */
@Composable
internal fun HomeTileGrid(tiles: List<HomeTileSpec>, modifier: Modifier = Modifier) {
    val style = MaterialTheme.typography.labelMedium
    val names = HOME_TILE_NAMES.map { stringResource(it) } + tiles.map { it.label }
    val measurer = rememberTextMeasurer()
    val widest = remember(measurer, style, names) {
        names.maxOfOrNull { measurer.measure(it, style, maxLines = 1, softWrap = false).size.width } ?: 0
    }
    BoxWithConstraints(modifier.fillMaxWidth().padding(horizontal = 2.dp)) {
        val columns = homeTileColumns(
            width = constraints.maxWidth,
            gap = with(LocalDensity.current) { TILE_GAP.roundToPx() },
            inset = with(LocalDensity.current) { (TILE_LABEL_INSET * 2).roundToPx() },
            widest = widest,
        )
        Column(verticalArrangement = Arrangement.spacedBy(TILE_GAP)) {
            tiles.chunked(columns).forEach { row ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(TILE_GAP)) {
                    row.forEach { tile ->
                        key(tile.id) { HomeTile(tile, Modifier.weight(1f)) }
                    }
                    repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}

/**
 * Tiles to a row [width] px wide with [gap] px between them: the most, up to [most], whose names
 * fit on one line with [inset] px beside them, the [widest] name included; one at the least.
 */
internal fun homeTileColumns(width: Int, gap: Int, inset: Int, widest: Int, most: Int = TILE_COLUMNS): Int =
    (most downTo 2).firstOrNull { n -> (width - gap * (n - 1)) / n - inset >= widest } ?: 1

/**
 * Home's pairs (the sound switches, Activity's two tiles) side by side in equal widths when each
 * fits there on one line, and otherwise one under another at the full width, [gap] apart, so a
 * name never breaks onto a second line.
 */
@Composable
internal fun HomeFitRow(modifier: Modifier = Modifier, gap: Dp = TILE_GAP, content: @Composable () -> Unit) {
    Layout(content = content, modifier = modifier) { measurables, constraints ->
        val gapPx = gap.roundToPx()
        val oneLine = measurables.map { it.maxIntrinsicWidth(Constraints.Infinity) }
        val width = if (constraints.hasBoundedWidth) {
            constraints.maxWidth
        } else {
            oneLine.sum() + gapPx * (oneLine.size - 1).coerceAtLeast(0)
        }
        val cell = homeFitCell(width, gapPx, measurables.size)
        if (homeFitsSideBySide(cell, oneLine)) {
            // As tall as the tallest, so the pair lines up.
            val height = (measurables.maxOfOrNull { it.minIntrinsicHeight(cell) } ?: 0).coerceAtMost(constraints.maxHeight)
            val placeables = measurables.map {
                it.measure(Constraints(minWidth = cell, maxWidth = cell, minHeight = height, maxHeight = constraints.maxHeight))
            }
            val rowHeight = placeables.maxOfOrNull { it.height } ?: 0
            layout(width, rowHeight) {
                placeables.forEachIndexed { index, placeable -> placeable.placeRelative(index * (cell + gapPx), 0) }
            }
        } else {
            val placeables = measurables.map { it.measure(Constraints(minWidth = width, maxWidth = width, maxHeight = constraints.maxHeight)) }
            val height = placeables.sumOf { it.height } + gapPx * (placeables.size - 1).coerceAtLeast(0)
            layout(width, height) {
                var y = 0
                placeables.forEach { placeable ->
                    placeable.placeRelative(0, y)
                    y += placeable.height + gapPx
                }
            }
        }
    }
}

/** Each of [count] items' width side by side in [width] px, [gap] px apart. */
internal fun homeFitCell(width: Int, gap: Int, count: Int): Int =
    if (count <= 1) width else (width - gap * (count - 1)) / count

/** Whether items whose one-line widths are [widths] px fit side by side in [cell] px each. */
internal fun homeFitsSideBySide(cell: Int, widths: List<Int>): Boolean = widths.all { it <= cell }

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
            .padding(horizontal = TILE_LABEL_INSET, vertical = 8.dp)
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
        val description = tile.description
        BasicText(
            tile.label,
            modifier = if (description != null) Modifier.clearAndSetSemantics { contentDescription = description } else Modifier,
            style = MaterialTheme.typography.labelMedium.copy(textAlign = TextAlign.Center),
            color = { lerp(labelIdle, labelActive, activeShown.value) },
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A tile two to a row ([HomeFitRow]): its glyph, its label and a count, for Activity. */
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
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(20.dp))
        Text(
            label,
            style = MaterialTheme.typography.labelLarge,
            color = scheme.onSurface,
            maxLines = 1,
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
 * PC as the phone's, its audio played there over Bluetooth.
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
        HomeFitRow(
            modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = if (output == null) 4.dp else 0.dp),
        ) {
            HomeToggleButton(
                label = stringResource(R.string.home_tile_phone_speaker),
                icon = G7Icons.speaker,
                tint = tints.teal,
                checked = outputs.streamingToPhone,
                onCheckedChange = { on -> onSetPcAudio(if (on) 201 else 200, null) },
            )
            if (btMedia != null) {
                HomeToggleButton(
                    label = stringResource(R.string.home_tile_pc_speaker),
                    icon = G7Icons.bluetooth,
                    tint = tints.blue,
                    checked = btMedia,
                    onCheckedChange = onSetBtMedia,
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
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
    }
}

/** The Record tile's text: [label] on the tile, and [description] for a screen reader while it records. */
internal class RecordTileText(val label: String, val description: String?)

/** "Record", or while it records its time ("0:42"), which a screen reader hears as "Stop · 0:42". */
@Composable
internal fun recordTileText(capture: PcCaptureState): RecordTileText {
    val elapsed = rememberElapsed(capture.recordingSince)
    if (capture.recordingSince == null) return RecordTileText(stringResource(R.string.home_tile_record), null)
    val time = PcCaptureFormat.elapsed(elapsed)
    return RecordTileText(time, stringResource(R.string.home_tile_stop_recording, time))
}
