// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.remote

import android.app.Activity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import app.g7link.android.R
import app.g7link.android.ble.PcBleLink
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.home.linkText
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.core.Link
import app.g7link.core.RemoteRoute

/**
 * The touchpad's connection to [device]: Auto, the network or Bluetooth, each
 * with how it's doing now and what it still needs. Picking Bluetooth (or Set up
 * under Auto) asks Android for what's missing first, then the PC.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ConnectionSheet(device: Device, core: Core, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val bluetoothOn by rememberBluetoothOn()
    // While Android asks: true picks Bluetooth once it's allowed, false sets it up for Auto.
    var pending by rememberSaveable { mutableStateOf<Boolean?>(null) }
    val turnOn = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode != Activity.RESULT_OK) pending = null
    }
    val nearby = rememberNearbyDevices { granted ->
        // The PCs hear what this phone can do now.
        core.refreshNotificationAccess()
        if (pending != null) {
            if (!granted) {
                pending = null
            } else if (!PcBleLink.bluetoothOn(context)) {
                runCatching { turnOn.launch(turnOnBluetoothIntent()) }.onFailure { pending = null }
            }
        }
    }
    LifecycleResumeEffect(nearby) {
        nearby.refresh()
        onPauseOrDispose {}
    }

    fun bringUp(pick: Boolean) {
        if (pick) core.setRemoteRoute(device.id, RemoteRoute.BLUETOOTH)
        core.setUpBluetooth(device.id, ask = true)
    }

    // Once Android said yes and Bluetooth is on, the PC is asked.
    LaunchedEffect(pending, nearby.state, bluetoothOn) {
        val pick = pending ?: return@LaunchedEffect
        if (nextSetupStep(nearby.state, bluetoothOn) != SetupStep.Ready) return@LaunchedEffect
        pending = null
        bringUp(pick)
    }

    fun setUp(pick: Boolean) {
        when (nextSetupStep(nearby.state, bluetoothOn)) {
            SetupStep.Nothing -> Unit
            SetupStep.AskPermission -> {
                pending = pick
                nearby.ask()
            }
            SetupStep.OpenAppInfo -> nearby.openAppInfo()
            SetupStep.TurnOn -> {
                pending = pick
                runCatching { turnOn.launch(turnOnBluetoothIntent()) }.onFailure { pending = null }
            }
            SetupStep.Ready -> {
                pending = null
                bringUp(pick)
            }
        }
    }

    val rows = connectionRows(device, nearby.state, bluetoothOn)
    // Under Auto the row's buttons set Bluetooth up for Auto; otherwise they pick it.
    val pickOnSetUp = device.route != RemoteRoute.AUTO
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                stringResource(R.string.remote_connection_title, device.name),
                style = MaterialTheme.typography.titleLarge,
            )
            Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                RouteRow(
                    label = stringResource(R.string.remote_route_auto),
                    status = rows.auto,
                    selected = device.route == RemoteRoute.AUTO,
                    pcName = device.name,
                    onPick = { core.setRemoteRoute(device.id, RemoteRoute.AUTO) },
                    onAction = { setUp(pick = false) },
                )
                RouteRow(
                    label = networkLabel(rows.networkPath),
                    status = rows.network,
                    selected = device.route == RemoteRoute.NETWORK,
                    pcName = device.name,
                    onPick = { core.setRemoteRoute(device.id, RemoteRoute.NETWORK) },
                    onAction = {},
                )
                RouteRow(
                    label = stringResource(R.string.remote_path_bluetooth),
                    status = rows.bluetooth,
                    selected = device.route == RemoteRoute.BLUETOOTH,
                    pcName = device.name,
                    onPick = { setUp(pick = true) },
                    onAction = { setUp(pick = pickOnSetUp) },
                )
            }
            Column(
                modifier = Modifier.padding(horizontal = 12.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    stringResource(R.string.remote_connection_bt_covers),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    stringResource(R.string.remote_connection_bt_also, device.name),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

/** A radio row: the label, its status line under it, and maybe a button. Picking the picked one does nothing. */
@Composable
private fun RouteRow(
    label: String,
    status: RowStatus,
    selected: Boolean,
    pcName: String,
    onPick: () -> Unit,
    onAction: () -> Unit,
) {
    val scheme = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .clip(shape)
            .then(if (selected) Modifier.background(scheme.surfaceContainer) else Modifier)
            .selectable(selected = selected, enabled = status.enabled, role = Role.RadioButton) {
                if (!selected) onPick()
            }
            .padding(horizontal = 12.dp, vertical = 8.dp)
            .alpha(if (status.enabled) 1f else 0.45f),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        RadioButton(selected = selected, onClick = null, enabled = status.enabled)
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyLarge, color = scheme.onSurface)
            Text(rowText(status.text, pcName), style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant)
        }
        if (status.busy) {
            CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp, color = scheme.primary)
        }
        status.action?.let { action ->
            G7RowAction(label = actionLabel(action), onClick = onAction, accent = true)
        }
    }
}

/** The header's note in words. */
@Composable
internal fun remoteNoteText(note: RemoteNote, link: Link): String = when (note) {
    RemoteNote.AsHome -> linkText(link)
    RemoteNote.OverBluetooth -> stringResource(R.string.bt_media_pc_hint)
    is RemoteNote.Path -> {
        val path = pathName(note.path)
        when {
            note.auto && note.rttMs != null -> stringResource(R.string.remote_note_auto_rtt, path, note.rttMs)
            note.auto -> stringResource(R.string.remote_note_auto, path)
            note.rttMs != null -> stringResource(R.string.remote_note_path_rtt, path, note.rttMs)
            else -> path
        }
    }
    is RemoteNote.BluetoothComing -> stringResource(R.string.remote_note_bt_coming, pathName(note.meanwhile))
}

@Composable
private fun pathName(path: PathName): String = stringResource(
    when (path) {
        PathName.WiFi -> R.string.remote_path_wifi
        PathName.Usb -> R.string.remote_path_usb
        PathName.Away -> R.string.remote_path_away
        PathName.Bluetooth -> R.string.remote_path_bluetooth
    },
)

/** The network row's name: the network's path, or "Network" while it's down. */
@Composable
private fun networkLabel(path: PathName?): String =
    if (path == null) stringResource(R.string.remote_path_network) else pathName(path)

@Composable
private fun rowText(text: RowText, pcName: String): String = when (text) {
    is RowText.Rtt -> stringResource(R.string.remote_route_rtt, text.ms)
    RowText.Connected -> stringResource(R.string.connected)
    RowText.NotConnected -> stringResource(R.string.not_connected)
    RowText.Connecting -> stringResource(R.string.link_connecting)
    is RowText.Using -> stringResource(R.string.remote_route_using, pathName(text.path), text.rttMs)
    RowText.PicksFaster -> stringResource(R.string.remote_route_picks_faster)
    RowText.NeedsSetUp -> stringResource(R.string.remote_route_needs_set_up)
    RowText.NeedsPermission -> stringResource(R.string.remote_bt_needs_permission)
    RowText.NeedsAndroid12 -> stringResource(R.string.remote_bt_needs_android_12)
    RowText.BluetoothOff -> stringResource(R.string.remote_bt_off)
    RowText.AllowOnPc -> stringResource(R.string.remote_bt_allow_on_pc, pcName)
    RowText.WaitingForPc -> stringResource(R.string.remote_bt_waiting, pcName)
    RowText.PcSaidNo -> stringResource(R.string.remote_bt_declined, pcName)
    RowText.PcHasNoBluetooth -> stringResource(R.string.remote_bt_unavailable, pcName)
    RowText.PcBusy -> stringResource(R.string.remote_bt_busy, pcName)
    RowText.OutOfRange -> stringResource(R.string.remote_bt_out_of_range)
    RowText.ConnectOverNetworkFirst -> stringResource(R.string.remote_bt_connect_first)
    RowText.Failed -> stringResource(R.string.remote_bt_failed)
}

@Composable
private fun actionLabel(action: RowAction): String = stringResource(
    when (action) {
        RowAction.SetUp -> R.string.remote_route_set_up
        RowAction.Allow -> R.string.action_allow
        RowAction.AppInfo -> R.string.action_app_info
        RowAction.TurnOn -> R.string.action_turn_on
        RowAction.Ask -> R.string.remote_bt_ask
        RowAction.AskAgain -> R.string.remote_bt_ask_again
        RowAction.TryAgain -> R.string.action_try_again
    },
)
