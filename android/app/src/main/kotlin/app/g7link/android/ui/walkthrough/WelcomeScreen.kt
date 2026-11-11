// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.walkthrough

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import app.g7link.android.R
import app.g7link.android.calls.PhoneCalls
import app.g7link.android.contacts.PhoneContacts
import app.g7link.android.core.BackgroundAccess
import app.g7link.android.core.CoreState
import app.g7link.android.mirror.InputService
import app.g7link.android.notifications.NotificationListener
import app.g7link.android.photos.RecentPhotos
import app.g7link.android.sms.PhoneSms
import app.g7link.android.storage.PhoneStorage
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Mark
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.g7Tints

/** One thing this phone can allow, with how to ask for it. */
internal data class AccessItem(
    val id: String,
    val title: String,
    val sub: String,
    val icon: ImageVector,
    val tint: Color,
    val granted: Boolean,
    /** Android holds it back for apps installed from a file until allowed in App info. */
    val restrictable: Boolean,
    val onAllow: () -> Unit,
)

/**
 * The phone's permissions, in the order the first-run screen lists them. The
 * launchers are registered here, so call this once per screen.
 */
@Composable
internal fun rememberAccessItems(state: CoreState, onAccessChanged: () -> Unit): List<AccessItem> {
    val context = LocalContext.current
    val tints = g7Tints()
    var refresh by remember { mutableIntStateOf(0) }
    val postNotifGranted = remember(state.notificationAccess, refresh) {
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
    }
    val notif = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        refresh++
        onAccessChanged()
        if (!NotificationListener.hasAccess(context)) {
            runCatching { context.startActivity(NotificationListener.settingsIntent(context)) }
        }
    }
    val runtime = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        onAccessChanged()
    }
    return listOf(
        AccessItem(
            id = "notifications",
            title = stringResource(R.string.walkthrough_perm_notif_title),
            sub = stringResource(R.string.welcome_notif_sub),
            icon = G7Icons.bell,
            tint = tints.blue,
            granted = state.notificationAccess && postNotifGranted,
            restrictable = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU,
            onAllow = {
                if (!postNotifGranted) {
                    notif.launch(Manifest.permission.POST_NOTIFICATIONS)
                } else {
                    runCatching { context.startActivity(NotificationListener.settingsIntent(context)) }
                }
            },
        ),
        AccessItem(
            id = "sms_contacts",
            title = stringResource(R.string.walkthrough_perm_sms_title),
            sub = stringResource(R.string.welcome_sms_sub),
            icon = G7Icons.message,
            tint = tints.blue,
            granted = state.smsAccess && state.contactsAccess,
            restrictable = Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM,
            onAllow = { runtime.launch(PhoneSms.permissions + PhoneContacts.permissions) },
        ),
        AccessItem(
            id = "calls",
            title = stringResource(R.string.walkthrough_perm_calls_title),
            sub = stringResource(R.string.welcome_calls_sub),
            icon = G7Icons.phone,
            tint = tints.teal,
            granted = state.callAccess,
            restrictable = false,
            onAllow = { runtime.launch(PhoneCalls.permissions) },
        ),
        AccessItem(
            id = "photos",
            title = stringResource(R.string.walkthrough_perm_photos_title),
            sub = stringResource(R.string.welcome_photos_sub),
            icon = G7Icons.photo,
            tint = tints.coral,
            granted = state.photoAccess,
            restrictable = false,
            onAllow = { runtime.launch(RecentPhotos.permissions) },
        ),
        AccessItem(
            id = "storage",
            title = stringResource(R.string.home_files),
            sub = stringResource(R.string.welcome_files_sub),
            icon = G7Icons.folder,
            tint = tints.amber,
            granted = state.storageAllFilesAccess || state.storageSafFolders.isNotEmpty(),
            restrictable = false,
            onAllow = { runCatching { context.startActivity(PhoneStorage.allFilesAccessIntent(context)) } },
        ),
        AccessItem(
            id = "battery",
            title = stringResource(R.string.welcome_battery_title),
            sub = stringResource(R.string.welcome_battery_sub),
            icon = G7Icons.battery,
            tint = tints.green,
            granted = state.backgroundUnrestricted,
            restrictable = false,
            onAllow = { runCatching { context.startActivity(BackgroundAccess.requestIntent(context)) } },
        ),
        AccessItem(
            id = "accessibility",
            title = stringResource(R.string.welcome_control_title),
            sub = stringResource(R.string.welcome_control_sub),
            icon = G7Icons.pointer,
            tint = tints.pink,
            granted = state.inputAccess || state.elevated,
            restrictable = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU,
            onAllow = { runCatching { context.startActivity(InputService.settingsIntent()) } },
        ),
    )
}

/**
 * Rows of [items], each with Allow, or Allowed once granted. After Allow on a
 * setting Android holds back from apps installed from a file, a row says where
 * to allow it.
 */
@Composable
internal fun AccessRows(items: List<AccessItem>) {
    val context = LocalContext.current
    val tints = g7Tints()
    val sideloaded = remember { BackgroundAccess.isSideloaded(context) }
    // Rows whose Allow was tapped: App info's menu item appears only after a try.
    var attempted by rememberSaveable { mutableStateOf("") }
    items.forEach { item ->
        G7Row(
            label = item.title,
            sublabel = item.sub,
            icon = item.icon,
            tint = item.tint,
        ) {
            if (item.granted) {
                Text(
                    stringResource(R.string.access_allowed),
                    style = MaterialTheme.typography.labelLarge,
                    color = tints.teal,
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 7.dp),
                )
            } else {
                G7RowAction(
                    label = stringResource(R.string.action_allow),
                    onClick = {
                        if (item.id !in attempted.split(',')) attempted += ",${item.id}"
                        item.onAllow()
                    },
                )
            }
        }
    }
    val blocked = items.any { it.restrictable && sideloaded && !it.granted && it.id in attempted.split(',') }
    if (blocked) {
        G7Row(
            label = stringResource(R.string.walkthrough_restricted_title),
            sublabel = stringResource(R.string.walkthrough_restricted_step2),
            icon = G7Icons.shield,
            tint = tints.amber,
        ) {
            G7RowAction(
                label = stringResource(R.string.action_app_info),
                onClick = { runCatching { context.startActivity(BackgroundAccess.appDetailsIntent(context)) } },
            )
        }
    }
}

/**
 * First run: what this phone can allow, then pairing. Everything here can be
 * changed later in Settings.
 */
@Composable
fun WelcomeScreen(
    state: CoreState,
    onPair: () -> Unit,
    onAccessChanged: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val items = rememberAccessItems(state, onAccessChanged)
    Column(
        modifier = modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.safeDrawing),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 22.dp, end = 22.dp, top = 40.dp, bottom = 18.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            G7Mark(large = true)
            Column {
                Text(stringResource(R.string.app_name), style = MaterialTheme.typography.titleLarge)
                Text(
                    stringResource(R.string.welcome_tagline),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 10.dp),
        ) {
            G7SectionLabel(stringResource(R.string.welcome_allow_heading))
            AccessRows(items)
        }
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 16.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            G7PrimaryButton(label = stringResource(R.string.action_pair_with_pc), onClick = onPair)
            Text(
                stringResource(R.string.welcome_private),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}
