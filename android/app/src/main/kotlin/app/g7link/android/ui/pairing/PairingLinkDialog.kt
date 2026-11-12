// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.pairing

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import app.g7link.android.R

/**
 * Asks before pairing with a link. For a link another app opened: that the name in it is only
 * what the link says, and that pairing goes on to compare a fresh code on both screens (anyone
 * can send such a link, so its secret proves nothing).
 */
@Composable
fun PairingLinkDialog(
    link: String,
    fromOtherApp: Boolean,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    val name = remember(link) {
        android.net.Uri.parse(link).getQueryParameter("n")?.trim()?.takeIf { it.isNotEmpty() }
    } ?: stringResource(R.string.pair_link_unknown_pc)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(
                if (fromOtherApp) {
                    stringResource(R.string.pair_link_title_other_app)
                } else {
                    stringResource(R.string.pair_link_title, name)
                },
            )
        },
        text = if (fromOtherApp) {
            {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(stringResource(R.string.pair_link_from_other_app))
                    Text(
                        stringResource(R.string.pair_link_named, name),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(stringResource(R.string.pair_link_next_compare))
                }
            }
        } else {
            null
        },
        confirmButton = {
            TextButton(onClick = onConfirm) { Text(stringResource(R.string.pair_link_confirm)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.pair_link_cancel)) }
        },
    )
}
