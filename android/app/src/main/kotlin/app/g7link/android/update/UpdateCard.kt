// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.update

import android.content.Intent
import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.BuildConfig
import app.g7link.android.R
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SwitchRow
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.android.ui.theme.g7Tints

/** The update a state is about, if any. */
private val AppUpdater.State.update: AppUpdater.Update?
    get() = when (this) {
        is AppUpdater.State.Available -> update
        is AppUpdater.State.NeedsPermission -> update
        is AppUpdater.State.Downloading -> update
        is AppUpdater.State.Installing -> update
        is AppUpdater.State.Failed -> update
        else -> null
    }

/** "G7 Link 0.2.0 is available" on Home, while there's an update to talk about. */
@Composable
fun UpdateCard(updater: AppUpdater, modifier: Modifier = Modifier) {
    val state by updater.state.collectAsStateWithLifecycle()
    val update = state.update ?: return
    UpdatePanel(updater, state, update, modifier)
}

/** One update's status, progress and actions, for Home and Settings. */
@Composable
private fun UpdatePanel(updater: AppUpdater, state: AppUpdater.State, update: AppUpdater.Update, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val failure = (state as? AppUpdater.State.Failed)?.reason
    val status = when {
        state is AppUpdater.State.Downloading && state.waitingForNetwork -> stringResource(R.string.update_waiting_network)
        state is AppUpdater.State.Downloading && state.total > 0 -> stringResource(
            R.string.update_downloading_progress,
            Formatter.formatShortFileSize(context, state.done),
            Formatter.formatShortFileSize(context, state.total),
        )
        state is AppUpdater.State.Downloading -> stringResource(R.string.update_downloading)
        state is AppUpdater.State.Installing -> stringResource(R.string.update_installing)
        state is AppUpdater.State.NeedsPermission -> stringResource(R.string.update_not_allowed)
        failure == AppUpdater.Reason.Damaged -> stringResource(R.string.update_damaged)
        failure == AppUpdater.Reason.Install -> stringResource(R.string.update_install_failed)
        failure != null -> stringResource(R.string.update_offline)
        else -> null
    }
    Column(modifier.fillMaxWidth()) {
        G7Row(
            label = stringResource(R.string.update_title, update.title(context)),
            sublabel = status,
            icon = G7Icons.download,
            tint = g7Tints().lavender,
        )
        val progressModifier = Modifier.fillMaxWidth().padding(start = 48.dp, end = 12.dp, bottom = 8.dp)
        when {
            state is AppUpdater.State.Downloading && state.total > 0 && !state.waitingForNetwork -> LinearProgressIndicator(
                progress = { (state.done.toFloat() / state.total).coerceIn(0f, 1f) },
                modifier = progressModifier,
            )
            state is AppUpdater.State.Downloading || state is AppUpdater.State.Installing ->
                if (LocalReducedMotion.current) {
                    LinearProgressIndicator(progress = { 0.5f }, modifier = progressModifier)
                } else {
                    LinearProgressIndicator(progressModifier)
                }
        }
        Row(
            modifier = Modifier.padding(start = 48.dp, bottom = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            when (state) {
                is AppUpdater.State.NeedsPermission -> G7RowAction(
                    label = stringResource(R.string.action_allow),
                    onClick = { context.startActivity(updater.allowInstallsIntent()) },
                    accent = true,
                )
                is AppUpdater.State.Available, is AppUpdater.State.Failed -> G7RowAction(
                    label = stringResource(if (failure != null) R.string.action_try_again else R.string.action_update),
                    onClick = { updater.requestInstall(update)?.let(context::startActivity) },
                    accent = true,
                )
                else -> Unit
            }
            G7RowAction(
                label = stringResource(R.string.update_whats_new),
                onClick = { runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, update.notesUrl.toUri())) } },
            )
        }
    }
}

/**
 * About in Settings: the version, which checks for updates when tapped, and,
 * where this build updates itself, automatic installs and the channel.
 */
@Composable
fun CheckForUpdates(updater: AppUpdater) {
    val tints = g7Tints()
    if (!updater.enabled) {
        G7Row(
            label = stringResource(R.string.about_version, BuildConfig.VERSION_NAME),
            icon = G7Icons.info,
            tint = tints.muted,
        )
        return
    }
    val state by updater.state.collectAsStateWithLifecycle()
    val auto by updater.autoInstall.collectAsStateWithLifecycle()
    val channel by updater.channel.collectAsStateWithLifecycle()
    val update = state.update
    if (update != null) {
        UpdatePanel(updater, state, update, Modifier.fillMaxWidth())
    } else {
        G7Row(
            label = stringResource(R.string.about_version, BuildConfig.VERSION_NAME),
            icon = G7Icons.info,
            tint = tints.muted,
            note = when (state) {
                AppUpdater.State.UpToDate -> stringResource(R.string.update_up_to_date)
                is AppUpdater.State.Failed -> stringResource(R.string.update_check_failed_short)
                AppUpdater.State.Checking -> null
                else -> stringResource(R.string.update_check)
            },
            enabled = state !is AppUpdater.State.Checking,
            onClick = updater::checkNow,
        ) {
            if (state is AppUpdater.State.Checking) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            }
        }
    }
    G7SwitchRow(
        label = stringResource(R.string.update_auto_title),
        checked = auto,
        onCheckedChange = updater::setAutoInstall,
        icon = G7Icons.download,
        tint = tints.lavender,
    )
    G7Row(
        label = stringResource(R.string.update_channel_title),
        icon = G7Icons.refresh,
        tint = tints.muted,
        note = stringResource(
            if (channel == AppUpdater.Channel.Nightly) R.string.update_channel_nightly else R.string.update_channel_stable,
        ),
        onClick = {
            updater.setChannel(
                if (channel == AppUpdater.Channel.Nightly) AppUpdater.Channel.Stable else AppUpdater.Channel.Nightly,
            )
        },
    )
}
