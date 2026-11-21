// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.settings

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.theme.G7Row
import app.g7link.core.AwayReach
import app.g7link.core.PeerAddressResult
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** The longest address the core takes: a 253-byte name, ":65535" and brackets. */
private const val MAX_ADDRESS_INPUT = 261

/** How often the away-from-home line is read again while it shows. */
private const val REFRESH_MS = 3_000L

/** What the phone knows about reaching a PC: the address saved for it, and from away. */
private data class PcReach(val saved: String?, val away: AwayReach?)

/** What the dialog says under the field after a save that didn't close it. */
private data class SaveNote(@StringRes val text: Int, val name: String? = null, val error: Boolean = false)

/**
 * Settings › PCs › a PC: the address this phone reaches it at away from home (else
 * found by itself), and one line on how it's reached from away.
 */
@Composable
fun PcAddressRow(core: Core, device: Device) {
    var reach by remember(device.id) { mutableStateOf<PcReach?>(null) }
    var editing by remember(device.id) { mutableStateOf(false) }
    // Bumped after a save, so the loop below reads both again at once.
    var reload by remember(device.id) { mutableIntStateOf(0) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle

    LaunchedEffect(core, device.id, lifecycle, reload) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (true) {
                reach = PcReach(core.peerAddress(device.id), core.awayReach(device.id))
                delay(REFRESH_MS)
            }
        }
    }

    Column {
        G7Row(
            label = stringResource(R.string.pc_address_title),
            note = reach?.let { it.saved ?: stringResource(R.string.pc_address_auto) },
            onClick = { editing = true },
        )
        reach?.away?.let { away ->
            Text(
                stringResource(awayReachText(away)),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 12.dp, end = 12.dp, bottom = 6.dp),
            )
        }
    }

    if (editing) {
        PcAddressDialog(
            core = core,
            device = device,
            saved = reach?.saved,
            onSaved = { reload++ },
            onDismiss = { editing = false },
        )
    }
}

/** Where to reach [device]: a Tailscale address or name, with an optional port. */
@Composable
private fun PcAddressDialog(
    core: Core,
    device: Device,
    saved: String?,
    onSaved: () -> Unit,
    onDismiss: () -> Unit,
) {
    var text by remember { mutableStateOf(saved.orEmpty()) }
    var stored by remember { mutableStateOf(saved != null) }
    var busy by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<SaveNote?>(null) }
    val scope = rememberCoroutineScope()

    fun submit(input: String) {
        if (busy) return
        busy = true
        note = null
        scope.launch {
            try {
                when (core.setPeerAddress(device.id, input)) {
                    PeerAddressResult.READY, PeerAddressResult.CLEARED -> {
                        onSaved()
                        onDismiss()
                    }
                    PeerAddressResult.NOT_FOUND -> {
                        stored = true
                        onSaved()
                        note = SaveNote(R.string.pc_address_not_found, name = input.trim())
                    }
                    PeerAddressResult.NOT_ALLOWED -> {
                        stored = true
                        onSaved()
                        note = SaveNote(R.string.pc_address_not_allowed)
                    }
                    PeerAddressResult.INVALID -> {
                        note = SaveNote(R.string.pc_address_invalid, error = true)
                    }
                    null -> {
                        note = SaveNote(R.string.pc_address_failed, error = true)
                    }
                }
            } finally {
                busy = false
            }
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
        title = { Text(stringResource(R.string.pc_address_dialog_title, device.name)) },
        confirmButton = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (busy) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(18.dp),
                        strokeWidth = 2.dp,
                        color = MaterialTheme.colorScheme.primary,
                    )
                }
                TextButton(onClick = { submit(text) }, enabled = !busy && text.isNotBlank()) {
                    Text(stringResource(R.string.pc_address_save))
                }
            }
        },
        dismissButton = {
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                if (stored) {
                    TextButton(onClick = { submit("") }, enabled = !busy) {
                        Text(stringResource(R.string.data_action_clear))
                    }
                }
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
            }
        },
        text = {
            Column(Modifier.fillMaxWidth()) {
                Text(
                    stringResource(R.string.pc_address_hint),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                OutlinedTextField(
                    value = text,
                    onValueChange = {
                        text = it.take(MAX_ADDRESS_INPUT)
                        note = null
                    },
                    modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                    singleLine = true,
                    placeholder = { Text(stringResource(R.string.pc_address_placeholder)) },
                    keyboardOptions = KeyboardOptions(
                        capitalization = KeyboardCapitalization.None,
                        autoCorrectEnabled = false,
                        keyboardType = KeyboardType.Uri,
                        imeAction = ImeAction.Done,
                    ),
                    keyboardActions = KeyboardActions(onDone = {
                        if (text.isNotBlank()) submit(text)
                    }),
                )
                note?.let {
                    val scheme = MaterialTheme.colorScheme
                    Text(
                        if (it.name != null) stringResource(it.text, it.name) else stringResource(it.text),
                        style = MaterialTheme.typography.bodySmall,
                        color = if (it.error) scheme.error else scheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
            }
        },
    )
}

@StringRes
private fun awayReachText(away: AwayReach): Int = when (away) {
    AwayReach.TAILSCALE -> R.string.away_reach_tailscale
    AwayReach.MOBILE_DATA -> R.string.away_reach_mobile_data
    AwayReach.TAILSCALE_OFF -> R.string.away_reach_tailscale_off
    AwayReach.HOME_ONLY -> R.string.away_reach_home_only
}
