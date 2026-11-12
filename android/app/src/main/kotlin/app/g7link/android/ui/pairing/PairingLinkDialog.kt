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
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.ui.theme.LocalAppFonts

/**
 * Asks before pairing with a link: the PC's code to compare (the real PC
 * shows it beside its QR code), and, for a link another app opened, that
 * the name in it is only what the link says.
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
    val code = remember(link) { runCatching { app.g7link.core.pairingLinkCode(link) }.getOrNull() }
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
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (fromOtherApp) {
                    Text(stringResource(R.string.pair_link_from_other_app))
                    Text(
                        stringResource(R.string.pair_link_named, name),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (code != null) {
                    val spaced = "${code.take(3)} ${code.drop(3)}"
                    Text(stringResource(R.string.pair_link_check_code))
                    Text(
                        spaced,
                        style = MaterialTheme.typography.headlineMedium,
                        fontFamily = LocalAppFonts.current.mono,
                        fontWeight = FontWeight.Medium,
                        modifier = Modifier.semantics { contentDescription = code.toList().joinToString(" ") },
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onConfirm) { Text(stringResource(R.string.pair_link_confirm)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.pair_link_cancel)) }
        },
    )
}
