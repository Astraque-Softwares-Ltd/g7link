// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.settings

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7Switch
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.glassModalContainerColor
import app.g7link.core.RelayEdit
import app.g7link.core.RelayEditResult
import app.g7link.core.RelayProbe
import app.g7link.core.RelayRegion
import app.g7link.core.RelayServer
import app.g7link.core.RelaySnapshot
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** The relays this phone may use away from home, the ones in use, and how each answers. */
@Composable
fun RelaySettingsDialog(core: Core, awayMode: Boolean, onDismiss: () -> Unit) {
    var snapshot by remember { mutableStateOf<RelaySnapshot?>(null) }
    var probes by remember { mutableStateOf(emptyMap<String, RelayProbe>()) }
    var checking by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf(false) }
    var address by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<RelayEditResult?>(null) }
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle

    LaunchedEffect(core, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (true) {
                snapshot = core.relaySnapshot()
                delay(2_000)
            }
        }
    }

    fun edit(url: String, action: RelayEdit) {
        if (editing) return
        editing = true
        scope.launch {
            try {
                val result = core.editRelay(url, action)
                if (result == RelayEditResult.SAVED) {
                    error = null
                    if (action == RelayEdit.ADD) address = ""
                    if (action == RelayEdit.REMOVE) probes = probes - url
                    snapshot = core.relaySnapshot()
                } else {
                    error = result
                }
            } finally {
                editing = false
            }
        }
    }

    fun check() {
        if (checking) return
        checking = true
        scope.launch {
            try {
                probes = core.checkRelays().associateBy { it.url }
            } finally {
                checking = false
            }
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = glassModalContainerColor(),
        title = { Text(stringResource(R.string.relay_settings_title)) },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_done)) } },
        dismissButton = {
            TextButton(onClick = { check() }, enabled = snapshot != null && !checking) {
                Text(stringResource(if (checking) R.string.relay_checking else R.string.relay_check_all))
            }
        },
        text = {
            val current = snapshot
            if (current == null) {
                Muted(stringResource(R.string.relay_starting))
            } else {
                Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
                    if (!awayMode) Muted(stringResource(R.string.relay_away_off))

                    G7SectionLabel(stringResource(R.string.relay_home))
                    if (current.home.isEmpty()) Muted(stringResource(R.string.relay_none))
                    current.home.forEach { relay ->
                        val state = when {
                            relay.connected -> R.string.connected
                            relay.failed -> R.string.relay_failed
                            else -> R.string.link_connecting
                        }
                        G7Row(label = relay.url, note = stringResource(state))
                    }

                    G7SectionLabel(stringResource(R.string.relay_device_routes))
                    if (current.routes.isEmpty()) Muted(stringResource(R.string.relay_none))
                    current.routes.forEach { route ->
                        G7Row(label = route.device, sublabel = route.url)
                    }

                    G7SectionLabel(stringResource(R.string.relay_servers))
                    current.servers.forEach { server ->
                        RelayServerRow(
                            server = server,
                            probe = probes[server.url],
                            checking = checking,
                            onEdit = { edit(server.url, it) },
                        )
                    }
                    Row(
                        Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        OutlinedTextField(
                            value = address,
                            onValueChange = {
                                address = it.take(1024)
                                error = null
                            },
                            modifier = Modifier.weight(1f),
                            singleLine = true,
                            label = { Text(stringResource(R.string.relay_address)) },
                            placeholder = { Text("https://relay.example.com") },
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = {
                                if (address.isNotBlank()) edit(address, RelayEdit.ADD)
                            }),
                        )
                        G7RowAction(
                            label = stringResource(R.string.relay_add),
                            onClick = { edit(address, RelayEdit.ADD) },
                            enabled = !editing && address.isNotBlank(),
                        )
                    }
                    error?.let {
                        Text(
                            stringResource(editError(it)),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                        )
                    }
                }
            }
        },
    )
}

/** A relay: its address, who runs it and where, its last check, and whether this phone may use it. */
@Composable
private fun RelayServerRow(
    server: RelayServer,
    probe: RelayProbe?,
    checking: Boolean,
    onEdit: (RelayEdit) -> Unit,
) {
    val rtt = probe?.rttMs
    val note = when {
        checking -> stringResource(R.string.relay_checking)
        probe == null -> null
        rtt != null -> stringResource(R.string.relay_rtt, rtt.toInt())
        probe.timedOut -> stringResource(R.string.relay_timed_out)
        else -> stringResource(R.string.relay_unreachable)
    }
    val region = stringResource(regionTitle(server.region))
    G7Row(
        label = server.url,
        sublabel = if (server.provider.isEmpty()) region else "${server.provider} · $region",
        note = note,
        role = Role.Switch,
        onClick = { onEdit(if (server.enabled) RelayEdit.DISABLE else RelayEdit.ENABLE) },
    ) {
        if (server.region == RelayRegion.CUSTOM) {
            G7IconButton(
                icon = G7Icons.trash,
                contentDescription = stringResource(R.string.timeline_action_remove),
                onClick = { onEdit(RelayEdit.REMOVE) },
                tint = g7Tints().red,
            )
        }
        G7Switch(checked = server.enabled, onCheckedChange = null)
    }
}

@Composable
private fun Muted(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
    )
}

@StringRes
private fun regionTitle(region: RelayRegion): Int = when (region) {
    RelayRegion.NORTH_AMERICA_EAST -> R.string.relay_region_na_east
    RelayRegion.NORTH_AMERICA_WEST -> R.string.relay_region_na_west
    RelayRegion.EUROPE -> R.string.relay_region_europe
    RelayRegion.ASIA_PACIFIC -> R.string.relay_region_asia_pacific
    RelayRegion.CUSTOM -> R.string.relay_region_custom
}

@StringRes
private fun editError(result: RelayEditResult): Int = when (result) {
    RelayEditResult.INVALID_ADDRESS -> R.string.relay_invalid_address
    RelayEditResult.DUPLICATE -> R.string.relay_duplicate
    RelayEditResult.LIMIT -> R.string.relay_limit
    else -> R.string.relay_save_failed
}
