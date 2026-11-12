// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.settings

import app.g7link.android.ui.theme.G7Slider
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.calls.PhoneCalls
import app.g7link.android.clipboard.PhoneClipboard
import app.g7link.android.companion.PcCompanion
import app.g7link.android.contacts.PhoneContacts
import app.g7link.android.core.BackgroundAccess
import app.g7link.android.core.CoreState
import app.g7link.android.core.Device
import app.g7link.android.elevated.Elevated
import app.g7link.android.elevated.ElevatedMethod
import app.g7link.android.elevated.PairingNotification
import app.g7link.android.elevated.ShizukuElevated
import app.g7link.android.mirror.InputService
import app.g7link.android.notifications.NotificationListener
import app.g7link.android.photos.RecentPhotos
import app.g7link.android.sms.PhoneSms
import app.g7link.android.storage.PhoneStorage
import app.g7link.android.toggles.PhoneToggles
import app.g7link.android.ui.AppLanguage
import app.g7link.android.ui.guide.GuideHighlightTarget
import app.g7link.android.ui.theme.Appearance
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7SectionLabel
import app.g7link.android.ui.theme.G7SwitchRow
import app.g7link.android.ui.theme.G7Tabs
import app.g7link.android.ui.theme.LocalGlassTheme
import app.g7link.android.ui.theme.LocalQuietTheme
import app.g7link.android.ui.theme.Tokens
import app.g7link.android.ui.theme.g7Tints
import app.g7link.android.ui.theme.glassModalBlur
import app.g7link.android.ui.theme.glassModalContainerColor
import app.g7link.android.update.AppUpdater
import app.g7link.android.update.CheckForUpdates
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Where each group sits in the list, for jumping to it from the Guide. */
private object Items {
    const val DEVICES = 1
    const val ACCESS = 2
    const val CONNECTION = 3
    const val CLIPBOARD = 4
    const val DATA = 7
}

@Composable
fun SettingsScreen(
    state: CoreState,
    appearance: Appearance,
    onAppearance: ((Appearance) -> Appearance) -> Unit,
    touchpadSensitivity: Float,
    onTouchpadSensitivity: (Float) -> Unit,
    onUnpair: (String) -> Unit,
    onPairNew: () -> Unit,
    onAccessChanged: () -> Unit,
    updater: AppUpdater,
    onSetStorageEnabled: (pcId: String, enabled: Boolean) -> Unit = { _, _ -> },
    onSetMicEnabled: (pcId: String, enabled: Boolean) -> Unit = { _, _ -> },
    onSetBtMedia: (pcId: String, on: Boolean) -> Unit = { _, _ -> },
    startWithoutAsking: Set<String> = emptySet(),
    onSetStartWithoutAsking: (pcId: String, on: Boolean) -> Unit = { _, _ -> },
    onAddSafFolder: (Uri) -> Unit = {},
    onRemoveSafFolder: (String) -> Unit = {},
    onSetClipboardHistoryEnabled: (Boolean) -> Unit = {},
    onClearClipboardHistory: () -> Unit = {},
    suggestClipboardActions: Boolean = true,
    onSetSuggestClipboardActions: (Boolean) -> Unit = {},
    volumeKeysControlPc: Boolean = true,
    onSetVolumeKeysControlPc: (Boolean) -> Unit = {},
    backgroundServiceEnabled: Boolean = true,
    onToggleBackgroundService: (Boolean) -> Unit = {},
    onSetTimelineRetentionDays: (UInt) -> Unit = {},
    onClearTimeline: () -> Unit = {},
    onClearMessageCache: () -> Unit = {},
    onClearPhotoCache: () -> Unit = {},
    onClearReceivedFileHistory: () -> Unit = {},
    onClearEverything: () -> Unit = {},
    onToggleLocalSend: (Boolean) -> Unit = {},
    onToggleAwayMode: (Boolean) -> Unit = {},
    onOpenRelaySettings: () -> Unit = {},
    onOpenWalkthrough: () -> Unit = {},
    highlightTarget: GuideHighlightTarget? = null,
    onHighlightConsumed: () -> Unit = {},
    modifier: Modifier = Modifier,
) {
    var confirmUnpair by remember { mutableStateOf<Device?>(null) }
    var confirmClearEverything by remember { mutableStateOf(false) }
    var pickLanguage by remember { mutableStateOf(false) }
    var openDeviceId by rememberSaveable { mutableStateOf<String?>(null) }
    var powerOpen by rememberSaveable { mutableStateOf(false) }
    var pulsing by remember { mutableStateOf<String?>(null) }
    var jumpToControl by remember { mutableStateOf(0) }
    val isGlass = LocalGlassTheme.current
    val quiet = LocalQuietTheme.current
    val tints = g7Tints()
    val anyDialogOpen = confirmUnpair != null || confirmClearEverything

    // Keep a screen of rows composed on each side, so scrolling back does not rebuild them.
    @OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
    val listState = androidx.compose.foundation.lazy.rememberLazyListState(
        cacheWindow = androidx.compose.foundation.lazy.layout.LazyLayoutCacheWindow(aheadFraction = 1f, behindFraction = 1f),
    )

    LaunchedEffect(jumpToControl) {
        if (jumpToControl == 0) return@LaunchedEffect
        powerOpen = true
        pulsing = "control"
        delay(60)
        runCatching { listState.animateScrollToItem(Items.ACCESS) }
        delay(2200)
        if (pulsing == "control") pulsing = null
    }

    LaunchedEffect(highlightTarget) {
        val target = highlightTarget ?: return@LaunchedEffect
        onHighlightConsumed()
        val (key, index) = when (target) {
            GuideHighlightTarget.SETTINGS_DEVICES -> "devices" to Items.DEVICES
            GuideHighlightTarget.SETTINGS_NOTIFICATIONS -> "notifications" to Items.ACCESS
            GuideHighlightTarget.SETTINGS_CONTROL -> "control" to Items.ACCESS
            GuideHighlightTarget.SETTINGS_CALLS_CONTACTS -> "calls_contacts" to Items.ACCESS
            GuideHighlightTarget.SETTINGS_PHOTOS_STORAGE -> "photos" to Items.ACCESS
            GuideHighlightTarget.SETTINGS_BACKGROUND -> "background" to Items.CONNECTION
            GuideHighlightTarget.SETTINGS_LOCALSEND -> "localsend" to Items.CONNECTION
            GuideHighlightTarget.SETTINGS_CLIPBOARD -> "clipboard" to Items.CLIPBOARD
            GuideHighlightTarget.SETTINGS_DATA_STORAGE -> "data_storage" to Items.DATA
            else -> return@LaunchedEffect
        }
        if (key == "control") powerOpen = true
        pulsing = key
        delay(60)
        runCatching { listState.animateScrollToItem(index) }
        delay(2200)
        if (pulsing == key) pulsing = null
    }

    LazyColumn(
        state = listState,
        modifier = modifier.glassModalBlur(isGlass && anyDialogOpen),
        contentPadding = PaddingValues(start = 10.dp, top = 8.dp, end = 10.dp, bottom = if (quiet) 16.dp else 104.dp),
    ) {
        item(key = "header", contentType = "header") {
            Text(
                stringResource(R.string.settings_title),
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.padding(start = 12.dp, top = 12.dp, bottom = 4.dp),
            )
        }

        item(key = "devices", contentType = "group") {
            val context = LocalContext.current
            Column {
                G7SectionLabel(stringResource(R.string.settings_devices))
                state.devices.forEach { device ->
                    val open = openDeviceId == device.id
                    G7Row(
                        label = device.name,
                        icon = if (device.isAndroid) G7Icons.phone else G7Icons.monitor,
                        tint = tints.lavender,
                        note = stringResource(if (device.online) R.string.connected else R.string.not_connected),
                        highlighted = pulsing == "devices",
                        onClick = { openDeviceId = if (open) null else device.id },
                    ) { Chevron(open) }
                    if (open) {
                        Column(Modifier.padding(start = 36.dp)) {
                            G7SwitchRow(
                                label = stringResource(R.string.storage_pc_toggle),
                                checked = device.storageEnabled,
                                onCheckedChange = { enabled ->
                                    onSetStorageEnabled(device.id, enabled)
                                    if (enabled && !state.storageAllFilesAccess && state.storageSafFolders.isEmpty()) {
                                        runCatching { context.startActivity(PhoneStorage.allFilesAccessIntent(context)) }
                                    }
                                },
                            )
                            if (!device.isAndroid) {
                                G7SwitchRow(
                                    label = stringResource(R.string.mic_pc_toggle),
                                    checked = device.micEnabled,
                                    onCheckedChange = { enabled -> onSetMicEnabled(device.id, enabled) },
                                )
                                G7SwitchRow(
                                    label = stringResource(R.string.bt_media_pc_toggle, device.name),
                                    sublabel = stringResource(R.string.bt_media_pc_hint),
                                    checked = device.btMedia,
                                    onCheckedChange = { on -> onSetBtMedia(device.id, on) },
                                )
                                G7SwitchRow(
                                    label = stringResource(R.string.start_without_asking_toggle),
                                    sublabel = stringResource(R.string.start_without_asking_hint),
                                    checked = device.id in startWithoutAsking,
                                    onCheckedChange = { on -> onSetStartWithoutAsking(device.id, on) },
                                )
                            }
                            G7Row(
                                label = stringResource(R.string.action_unpair),
                                icon = G7Icons.trash,
                                tint = tints.red,
                                onClick = { confirmUnpair = device },
                            )
                        }
                    }
                }
                G7Row(
                    label = stringResource(R.string.action_pair_new),
                    icon = G7Icons.plus,
                    tint = tints.muted,
                    onClick = onPairNew,
                )
            }
        }

        item(key = "access", contentType = "group") {
            Column {
                G7SectionLabel(stringResource(R.string.settings_access))
                G7Row(
                    label = stringResource(R.string.walkthrough_done_power_label),
                    icon = G7Icons.bolt,
                    tint = tints.amber,
                    note = stringResource(
                        when {
                            state.elevated -> R.string.walkthrough_power_elevated_title
                            state.inputAccess -> R.string.walkthrough_power_assist_title
                            else -> R.string.walkthrough_power_standard_title
                        },
                    ),
                    highlighted = pulsing == "control",
                    onClick = { powerOpen = !powerOpen },
                ) { Chevron(powerOpen) }
                if (powerOpen) {
                    Column(Modifier.padding(start = 36.dp)) {
                        ControlAccess(state.inputAccess)
                        ElevatedAccess()
                    }
                }
                NotificationAccess(state.notificationAccess, highlighted = pulsing == "notifications")
                RuntimePermissionAccess(
                    title = stringResource(R.string.calls_title),
                    icon = G7Icons.call,
                    tint = tints.teal,
                    granted = state.callAccess,
                    permissions = PhoneCalls.permissions,
                    onAccessChanged = onAccessChanged,
                    highlighted = pulsing == "calls_contacts",
                )
                RuntimePermissionAccess(
                    title = stringResource(R.string.contacts_title),
                    icon = G7Icons.person,
                    tint = tints.blue,
                    granted = state.contactsAccess,
                    permissions = PhoneContacts.permissions,
                    onAccessChanged = onAccessChanged,
                    highlighted = pulsing == "calls_contacts",
                )
                RuntimePermissionAccess(
                    title = stringResource(R.string.sms_title),
                    icon = G7Icons.message,
                    tint = tints.blue,
                    granted = state.smsAccess,
                    permissions = PhoneSms.permissions,
                    onAccessChanged = onAccessChanged,
                    highlighted = pulsing == "calls_contacts",
                )
                PhotosAccess(
                    granted = state.photoAccess,
                    partial = state.photoPartialAccess,
                    onAccessChanged = onAccessChanged,
                    highlighted = pulsing == "photos",
                )
                StorageAccess(
                    allFiles = state.storageAllFilesAccess,
                    safFolders = state.storageSafFolders,
                    onAddSafFolder = onAddSafFolder,
                    onRemoveSafFolder = onRemoveSafFolder,
                    highlighted = pulsing == "photos",
                )
                DndAccess(state.dndAccess)
                WriteSettingsAccess(state.writeSettingsAccess || state.elevated)
                BankingPauseAccess()
            }
        }

        item(key = "connection", contentType = "group") {
            Column {
                G7SectionLabel(stringResource(R.string.settings_connection))
                G7SwitchRow(
                    label = stringResource(R.string.guide_away_title),
                    checked = state.awayMode,
                    onCheckedChange = onToggleAwayMode,
                    icon = G7Icons.signal,
                    tint = tints.teal,
                )
                G7Row(
                    label = stringResource(R.string.relay_settings_title),
                    icon = G7Icons.globe,
                    tint = tints.blue,
                    onClick = onOpenRelaySettings,
                )
                G7SwitchRow(
                    label = stringResource(R.string.settings_run_in_background),
                    checked = backgroundServiceEnabled,
                    onCheckedChange = onToggleBackgroundService,
                    icon = G7Icons.always,
                    tint = tints.lavender,
                    highlighted = pulsing == "background",
                )
                BackgroundBatteryAccess(state.backgroundUnrestricted, highlighted = pulsing == "background")
                CompanionWakeAccess(
                    associated = state.companionAssociated,
                    devices = state.devices,
                    onAccessChanged = onAccessChanged,
                )
                G7SwitchRow(
                    label = stringResource(R.string.settings_localsend_title),
                    checked = state.localsendEnabled,
                    onCheckedChange = onToggleLocalSend,
                    icon = G7Icons.upload,
                    tint = tints.green,
                    highlighted = pulsing == "localsend",
                )
            }
        }

        item(key = "clipboard", contentType = "group") {
            val context = LocalContext.current
            var syncPasswords by remember { mutableStateOf(PhoneClipboard.syncPasswords(context)) }
            Column {
                G7SectionLabel(stringResource(R.string.settings_clipboard_title))
                // Elevated's own row knows every way to start it.
                G7Row(
                    label = stringResource(R.string.settings_background_clipboard_title),
                    icon = G7Icons.clipboard,
                    tint = tints.lavender,
                    highlighted = pulsing == "clipboard",
                    onClick = { jumpToControl++ },
                ) {
                    if (state.elevated) {
                        AllowedText()
                    } else {
                        G7RowAction(label = stringResource(R.string.elevated_set_up), onClick = { jumpToControl++ })
                    }
                }
                G7SwitchRow(
                    label = stringResource(R.string.settings_clipboard_passwords),
                    sublabel = stringResource(R.string.settings_clipboard_passwords_hint),
                    checked = syncPasswords,
                    onCheckedChange = {
                        syncPasswords = it
                        PhoneClipboard.setSyncPasswords(context, it)
                    },
                    icon = G7Icons.shield,
                    tint = tints.amber,
                )
                G7SwitchRow(
                    label = stringResource(R.string.settings_clipboard_suggestions),
                    checked = suggestClipboardActions,
                    onCheckedChange = onSetSuggestClipboardActions,
                    icon = G7Icons.lightbulb,
                    tint = tints.blue,
                )
            }
        }

        item(key = "remote", contentType = "group") {
            val sensitivityTitle = stringResource(R.string.settings_touchpad_sensitivity)
            Column {
                G7SectionLabel(stringResource(R.string.settings_remote))
                G7Row(
                    label = sensitivityTitle,
                    icon = G7Icons.pointer,
                    tint = tints.teal,
                    note = stringResource(R.string.settings_touchpad_sensitivity_value, touchpadSensitivity),
                )
                G7Slider(
                    value = touchpadSensitivity,
                    onValueChange = onTouchpadSensitivity,
                    valueRange = 0.5f..2.5f,
                    tint = tints.teal,
                    trackColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                    modifier = Modifier
                        .padding(start = 48.dp, end = 12.dp)
                        .semantics { contentDescription = sensitivityTitle },
                )
                G7SwitchRow(
                    label = stringResource(R.string.settings_volume_keys_control_pc),
                    checked = volumeKeysControlPc,
                    onCheckedChange = onSetVolumeKeysControlPc,
                    icon = G7Icons.volume,
                    tint = tints.teal,
                )
            }
        }

        item(key = "appearance", contentType = "group") {
            Column {
                G7SectionLabel(stringResource(R.string.settings_appearance))
                ChoiceRow(
                    label = stringResource(R.string.settings_theme),
                    icon = G7Icons.palette,
                    tint = tints.lavender,
                    options = listOf(
                        "quiet" to "G7",
                        "hive" to stringResource(R.string.theme_hive),
                        "bloom" to stringResource(R.string.theme_bloom),
                        "glass" to "Glass",
                        "graphite" to stringResource(R.string.theme_graphite),
                    ),
                    selected = appearance.theme,
                ) { value -> onAppearance { it.copy(theme = value) } }
                ChoiceRow(
                    label = stringResource(R.string.settings_mode),
                    icon = G7Icons.moon,
                    tint = tints.blue,
                    options = listOf(
                        "system" to stringResource(R.string.mode_system),
                        "light" to stringResource(R.string.mode_light),
                        "dark" to stringResource(R.string.mode_dark),
                    ),
                    selected = appearance.mode,
                ) { value -> onAppearance { it.copy(mode = value) } }
                if (AppLanguage.supported) {
                    // System and English, plus the language picked from the list (or More).
                    val context = LocalContext.current
                    val language = remember { AppLanguage.current(context) }
                    val other = AppLanguage.languages.firstOrNull { it.first == language && it.first != "en" }
                    ChoiceRow(
                        label = stringResource(R.string.settings_language),
                        icon = G7Icons.globe,
                        tint = tints.teal,
                        options = listOf(
                            "" to stringResource(R.string.mode_system),
                            "en" to "English",
                            (other ?: (MORE_LANGUAGES to stringResource(R.string.settings_language_more))),
                        ),
                        selected = language,
                    ) { value ->
                        if (value == "" || value == "en") AppLanguage.set(context, value) else pickLanguage = true
                    }
                }
                ThemeExtras(appearance = appearance, onAppearance = onAppearance)
            }
        }

        item(key = "data_storage", contentType = "group") {
            val highlighted = pulsing == "data_storage"
            Column {
                G7SectionLabel(stringResource(R.string.settings_data_storage_title))
                G7Row(
                    label = stringResource(R.string.data_notifications_title),
                    sublabel = stringResource(R.string.data_notifications_subtitle),
                    icon = G7Icons.bell,
                    tint = tints.muted,
                    highlighted = highlighted,
                )
                G7SwitchRow(
                    label = stringResource(R.string.data_clipboard_title),
                    sublabel = stringResource(R.string.data_clipboard_subtitle),
                    checked = state.clipboardHistoryEnabled,
                    onCheckedChange = onSetClipboardHistoryEnabled,
                    icon = G7Icons.clipboard,
                    tint = tints.muted,
                )
                if (state.clipboardHistory.isNotEmpty()) {
                    G7Row(
                        label = pluralStringResource(
                            R.plurals.data_clipboard_count,
                            state.clipboardHistory.size,
                            state.clipboardHistory.size,
                        ),
                        modifier = Modifier.padding(start = 36.dp),
                    ) {
                        G7RowAction(label = stringResource(R.string.data_action_clear), onClick = onClearClipboardHistory)
                    }
                }
                G7Row(
                    label = stringResource(R.string.data_timeline_title),
                    sublabel = stringResource(R.string.data_timeline_subtitle),
                    icon = G7Icons.history,
                    tint = tints.muted,
                ) {
                    if (state.timeline.isNotEmpty() || state.timelineTotal > 0u) {
                        G7RowAction(label = stringResource(R.string.data_action_clear), onClick = onClearTimeline)
                    }
                }
                Choice(
                    options = listOf(
                        "30" to stringResource(R.string.timeline_retention_30d),
                        "90" to stringResource(R.string.timeline_retention_90d),
                        "365" to stringResource(R.string.timeline_retention_1y),
                        "0" to stringResource(R.string.timeline_retention_all),
                    ),
                    selected = state.timelineRetentionDays.toString(),
                    onSelect = { value -> value.toUIntOrNull()?.let(onSetTimelineRetentionDays) },
                    modifier = Modifier.padding(start = 48.dp, end = 12.dp, bottom = 6.dp),
                )
                G7Row(
                    label = stringResource(R.string.data_messages_title),
                    sublabel = stringResource(R.string.data_messages_subtitle),
                    icon = G7Icons.message,
                    tint = tints.muted,
                ) {
                    if (state.cachedMmsFiles > 0) {
                        G7RowAction(label = stringResource(R.string.data_action_clear), onClick = onClearMessageCache)
                    }
                }
                G7Row(
                    label = stringResource(R.string.data_photos_title),
                    sublabel = stringResource(R.string.data_photos_subtitle),
                    icon = G7Icons.photo,
                    tint = tints.muted,
                ) {
                    if (state.cachedPhotoFiles > 0) {
                        G7RowAction(label = stringResource(R.string.data_action_clear), onClick = onClearPhotoCache)
                    }
                }
                G7Row(
                    label = stringResource(R.string.data_received_files_title),
                    sublabel = stringResource(R.string.data_received_files_subtitle),
                    icon = G7Icons.download,
                    tint = tints.muted,
                ) {
                    if (state.receivedFileRecords > 0 || state.transfers.isNotEmpty()) {
                        G7RowAction(label = stringResource(R.string.data_action_clear), onClick = onClearReceivedFileHistory)
                    }
                }
                G7Row(
                    label = stringResource(R.string.data_clear_everything_button),
                    sublabel = stringResource(R.string.data_clear_everything_hint),
                    icon = G7Icons.trash,
                    tint = tints.red,
                    onClick = { confirmClearEverything = true },
                )
            }
        }

        item(key = "about", contentType = "group") {
            Column {
                G7SectionLabel(stringResource(R.string.settings_about))
                CheckForUpdates(updater)
                G7Row(
                    label = stringResource(R.string.guide_open_walkthrough),
                    icon = G7Icons.guide,
                    tint = tints.muted,
                    onClick = onOpenWalkthrough,
                )
            }
        }
    }

    confirmUnpair?.let { device ->
        AlertDialog(
            onDismissRequest = { confirmUnpair = null },
            containerColor = glassModalContainerColor(),
            title = { Text(stringResource(R.string.unpair_title, device.name)) },
            text = { Text(stringResource(R.string.unpair_text)) },
            confirmButton = {
                TextButton(onClick = {
                    onUnpair(device.id)
                    confirmUnpair = null
                    openDeviceId = null
                }) {
                    Text(stringResource(R.string.action_unpair))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmUnpair = null }) { Text(stringResource(R.string.action_cancel)) }
            },
        )
    }

    if (pickLanguage && AppLanguage.supported) {
        val context = LocalContext.current
        val language = remember { AppLanguage.current(context) }
        AlertDialog(
            onDismissRequest = { pickLanguage = false },
            containerColor = glassModalContainerColor(),
            title = { Text(stringResource(R.string.settings_language)) },
            text = {
                Column {
                    AppLanguage.languages.forEach { (tag, name) ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .selectable(selected = tag == language, role = Role.RadioButton) {
                                    pickLanguage = false
                                    AppLanguage.set(context, tag)
                                }
                                .padding(vertical = 12.dp),
                        ) {
                            RadioButton(selected = tag == language, onClick = null)
                            Spacer(Modifier.width(12.dp))
                            Text(name, style = MaterialTheme.typography.bodyLarge)
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { pickLanguage = false }) { Text(stringResource(R.string.action_cancel)) }
            },
        )
    }

    if (confirmClearEverything) {
        AlertDialog(
            onDismissRequest = { confirmClearEverything = false },
            containerColor = glassModalContainerColor(),
            title = { Text(stringResource(R.string.data_clear_everything_dialog_title)) },
            text = { Text(stringResource(R.string.data_clear_everything_dialog_body)) },
            confirmButton = {
                TextButton(onClick = {
                    onClearEverything()
                    confirmClearEverything = false
                }) {
                    Text(stringResource(R.string.data_clear_everything_confirm))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmClearEverything = false }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }
}

/** A theme's own options: Hive's comb, Graphite's black, Bloom and Glass's colors. */
@Composable
private fun ThemeExtras(appearance: Appearance, onAppearance: ((Appearance) -> Appearance) -> Unit) {
    if (appearance.theme == "hive") {
        G7SwitchRow(
            label = stringResource(R.string.settings_comb_motion),
            checked = appearance.combMotion,
            onCheckedChange = { on -> onAppearance { it.copy(combMotion = on) } },
        )
    }
    if (appearance.theme == "graphite" && appearance.mode != "light") {
        G7SwitchRow(
            label = stringResource(R.string.settings_amoled),
            checked = appearance.amoled,
            onCheckedChange = { on -> onAppearance { it.copy(amoled = on) } },
        )
    }
    if (appearance.theme != "bloom" && appearance.theme != "glass") return
    val artworkSync = appearance.seed == "artwork"
    G7SwitchRow(
        label = stringResource(R.string.settings_theme_artwork_color),
        checked = artworkSync,
        onCheckedChange = { on -> onAppearance { it.copy(seed = if (on) "artwork" else Tokens.DEFAULT_SEED) } },
    )
    if (artworkSync) return
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        G7SwitchRow(
            label = stringResource(R.string.settings_dynamic_color),
            checked = appearance.dynamicColor,
            onCheckedChange = { on -> onAppearance { it.copy(dynamicColor = on) } },
        )
    }
    val swatchesActive = !appearance.dynamicColor || Build.VERSION.SDK_INT < Build.VERSION_CODES.S
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Tokens.bloomSeeds.forEach { (name, seed) ->
            val isSelected = swatchesActive && appearance.seed == name
            val seedLabel = stringResource(
                when (name) {
                    "honey" -> R.string.settings_seed_honey
                    "lavender" -> R.string.settings_seed_lavender
                    "ocean" -> R.string.settings_seed_ocean
                    "sage" -> R.string.settings_seed_sage
                    "black" -> R.string.settings_seed_black
                    else -> R.string.settings_seed_honey
                },
            )
            val isBlackSeed = name == "black"
            val borderColor = when {
                isSelected -> MaterialTheme.colorScheme.onSurface
                isBlackSeed -> Color.White.copy(alpha = 0.28f)
                else -> MaterialTheme.colorScheme.outlineVariant
            }
            val borderWidth = when {
                isSelected -> 2.5.dp
                isBlackSeed -> 1.2.dp
                else -> 1.dp
            }
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .size(38.dp)
                    .drawBehind {
                        drawCircle(color = seed.color)
                        val strokePx = borderWidth.toPx()
                        drawCircle(
                            color = borderColor,
                            radius = (size.minDimension - strokePx) * 0.5f,
                            style = androidx.compose.ui.graphics.drawscope.Stroke(width = strokePx),
                        )
                    }
                    .clickable { onAppearance { it.copy(dynamicColor = false, seed = name) } }
                    .semantics {
                        role = Role.RadioButton
                        selected = isSelected
                        contentDescription = seedLabel
                    },
            ) {
                if (isSelected) {
                    Text(
                        text = "✓",
                        style = MaterialTheme.typography.labelLarge,
                        color = if (isBlackSeed) Color.White else Color.Black,
                    )
                }
            }
        }
    }
    // Pure black goes with the Black swatch.
    if (swatchesActive && appearance.seed == "black") {
        G7SwitchRow(
            label = stringResource(R.string.settings_amoled),
            checked = appearance.amoled,
            onCheckedChange = { on -> onAppearance { it.copy(amoled = on) } },
        )
    }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is android.content.ContextWrapper -> baseContext.findActivity()
    else -> null
}

private fun openAppInfoSettings(context: Context) {
    runCatching {
        context.startActivity(
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}

/** Shizuku's app, where it's started and where G7 Link is allowed. */
private fun openShizuku(context: Context) {
    context.packageManager.getLaunchIntentForPackage(ShizukuElevated.PACKAGE)
        ?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ?.let { runCatching { context.startActivity(it) } }
}

private fun openDeveloperOptionsSettings(context: Context) {
    val devIntent = Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    if (runCatching { context.startActivity(devIntent) }.isFailure) {
        runCatching {
            context.startActivity(Intent(Settings.ACTION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }
}

@Composable
private fun Chevron(open: Boolean) {
    Icon(
        imageVector = G7Icons.chevronDown,
        contentDescription = null,
        tint = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.size(18.dp).graphicsLayer { rotationZ = if (open) 180f else 0f },
    )
}

@Composable
private fun AllowedText() {
    Text(
        stringResource(R.string.access_allowed),
        style = MaterialTheme.typography.labelLarge,
        color = g7Tints().teal,
        modifier = Modifier.padding(horizontal = 12.dp, vertical = 7.dp),
    )
}

/**
 * Something Android must allow: Allowed (tap to manage it in Android's settings)
 * or an Allow button.
 */
@Composable
private fun AccessRow(
    label: String,
    icon: ImageVector,
    tint: Color,
    granted: Boolean,
    onAllow: () -> Unit,
    onManage: () -> Unit,
    sublabel: String? = null,
    highlighted: Boolean = false,
    allowLabel: String = stringResource(R.string.action_allow),
) {
    G7Row(
        label = label,
        icon = icon,
        tint = tint,
        sublabel = sublabel,
        highlighted = highlighted,
        onClick = if (granted) onManage else onAllow,
    ) {
        if (granted) AllowedText() else G7RowAction(label = allowLabel, onClick = onAllow)
    }
}

/**
 * Android holds some settings back for apps installed from a file: where to
 * allow them.
 */
@Composable
private fun RestrictedRow() {
    val context = LocalContext.current
    G7Row(
        label = stringResource(R.string.action_app_info),
        sublabel = stringResource(R.string.notifications_restricted),
        icon = G7Icons.shield,
        tint = g7Tints().muted,
        onClick = { openAppInfoSettings(context) },
    )
}

/** Notification access, which mirroring needs. Android shows the switch in its own settings. */
@Composable
private fun NotificationAccess(granted: Boolean, highlighted: Boolean) {
    val context = LocalContext.current
    val open = { runCatching { context.startActivity(NotificationListener.settingsIntent(context)) }.let {} }
    AccessRow(
        label = stringResource(R.string.notifications_title),
        icon = G7Icons.bell,
        tint = g7Tints().blue,
        granted = granted,
        onAllow = open,
        onManage = open,
        highlighted = highlighted,
    )
    if (!granted && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) RestrictedRow()
}

/** Unrestricted background execution and the phone maker's own battery settings. */
@Composable
private fun BackgroundBatteryAccess(unrestricted: Boolean, highlighted: Boolean) {
    val context = LocalContext.current
    val oemGuide = BackgroundAccess.oemGuideResId()?.let { stringResource(it) }
    AccessRow(
        label = stringResource(R.string.background_title),
        icon = G7Icons.battery,
        tint = g7Tints().green,
        granted = unrestricted,
        onAllow = { runCatching { context.startActivity(BackgroundAccess.requestIntent(context)) } },
        onManage = { runCatching { context.startActivity(BackgroundAccess.oemSettingsIntent(context)) } },
        highlighted = highlighted,
    )
    G7Row(
        label = stringResource(R.string.battery_open_oem_settings),
        sublabel = oemGuide,
        modifier = Modifier.padding(start = 36.dp),
        onClick = { runCatching { context.startActivity(BackgroundAccess.oemSettingsIntent(context)) } },
    )
}

/** Companion Device Manager association for waking when a PC BLE beacon is nearby. */
@Composable
private fun CompanionWakeAccess(
    associated: Boolean,
    devices: List<Device>,
    onAccessChanged: () -> Unit,
) {
    val context = LocalContext.current
    val chooserLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartIntentSenderForResult(),
    ) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            PcCompanion.startObservingPresence(context)
        }
        onAccessChanged()
    }
    val primaryPc = devices.firstOrNull { it.online } ?: devices.firstOrNull()
    val triggerAssociate = {
        val activity = context.findActivity()
        if (activity != null) {
            PcCompanion.requestAssociation(
                activity = activity,
                pcName = primaryPc?.name ?: "PC",
                pcDeviceId = primaryPc?.id,
                onResult = { onAccessChanged() },
                onLauncherFallback = { sender ->
                    runCatching {
                        chooserLauncher.launch(IntentSenderRequest.Builder(sender).build())
                    }
                },
            )
        }
    }
    AccessRow(
        label = stringResource(R.string.companion_wake_title),
        icon = G7Icons.monitor,
        tint = g7Tints().blue,
        granted = associated,
        onAllow = { triggerAssociate() },
        onManage = { triggerAssociate() },
        allowLabel = stringResource(R.string.companion_wake_associate),
    )
}

/** Do Not Disturb and silent ringer control from a PC. */
@Composable
private fun DndAccess(granted: Boolean) {
    val context = LocalContext.current
    val open = { runCatching { context.startActivity(PhoneToggles.dndSettingsIntent()) }.let {} }
    AccessRow(
        label = stringResource(R.string.dnd_access_title),
        icon = G7Icons.moon,
        tint = g7Tints().lavender,
        granted = granted,
        onAllow = open,
        onManage = open,
    )
}

/** Screen brightness control from a PC (Modify system settings, or Elevated). */
@Composable
private fun WriteSettingsAccess(granted: Boolean) {
    val context = LocalContext.current
    val open = { runCatching { context.startActivity(PhoneToggles.writeSettingsIntent(context)) }.let {} }
    AccessRow(
        label = stringResource(R.string.write_settings_title),
        icon = G7Icons.sun,
        tint = g7Tints().amber,
        granted = granted,
        onAllow = open,
        onManage = open,
    )
}

/** Assist: G7 Link's accessibility service, turned on in Android's settings. */
@Composable
private fun ControlAccess(on: Boolean) {
    val context = LocalContext.current
    val open = { runCatching { context.startActivity(InputService.settingsIntent()) }.let {} }
    AccessRow(
        label = stringResource(R.string.walkthrough_power_assist_title),
        sublabel = stringResource(R.string.control_title),
        icon = G7Icons.pointer,
        tint = g7Tints().pink,
        granted = on,
        onAllow = open,
        onManage = open,
    )
    if (!on && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) RestrictedRow()
}

/** Elevated, through the phone's own wireless debugging, Shizuku or root: real touch and keys. */
@Composable
private fun ElevatedAccess() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val tints = g7Tints()
    val state by Elevated.state.collectAsStateWithLifecycle()
    var steps by remember { mutableStateOf(false) }
    val shizukuInstalled = remember { ShizukuElevated.installed(context) }
    val failure = (state as? Elevated.State.Failed)?.reason
    val shizukuFailure = failure == Elevated.Reason.ShizukuNotRunning ||
        failure == Elevated.Reason.ShizukuDenied || failure == Elevated.Reason.ShizukuRoot
    // Through Shizuku, the way back is in Shizuku's app, not Developer options.
    val viaShizuku = Elevated.usesShizuku() || shizukuFailure
    val methods = remember { ElevatedMethod.available(shizukuInstalled, Elevated.isRootAvailable()) }
    var method by remember {
        mutableStateOf(if (Elevated.usesShizuku()) ElevatedMethod.Shizuku else ElevatedMethod.WirelessDebugging)
    }
    fun setUp() {
        when (method) {
            ElevatedMethod.WirelessDebugging -> {
                Elevated.useWirelessDebugging()
                steps = true
            }
            ElevatedMethod.Shizuku -> scope.launch {
                if (!Elevated.setUpWithShizuku() && !ShizukuElevated.running()) openShizuku(context)
            }
            ElevatedMethod.Root -> scope.launch { Elevated.setUpWithRoot() }
        }
    }
    val status = when (state) {
        Elevated.State.Waiting ->
            if (Elevated.usesShizuku()) R.string.elevated_shizuku_waiting else R.string.elevated_waiting
        Elevated.State.Starting -> R.string.elevated_starting
        is Elevated.State.Failed -> when ((state as Elevated.State.Failed).reason) {
            Elevated.Reason.NotPaired -> R.string.elevated_not_paired
            Elevated.Reason.RootDenied -> R.string.elevated_root_failed
            Elevated.Reason.ShizukuNotRunning -> R.string.elevated_shizuku_not_running
            Elevated.Reason.ShizukuDenied -> R.string.elevated_shizuku_denied
            Elevated.Reason.ShizukuRoot -> R.string.elevated_shizuku_root
            else -> R.string.elevated_failed
        }
        Elevated.State.Running -> when {
            Elevated.rootActive -> R.string.elevated_method_root
            Elevated.shizukuActive -> R.string.elevated_method_shizuku
            else -> R.string.elevated_method_wireless
        }
        else -> null
    }
    G7Row(
        label = stringResource(R.string.walkthrough_power_elevated_title),
        sublabel = status?.let { stringResource(it) },
        icon = G7Icons.bolt,
        tint = tints.amber,
    ) {
        when (state) {
            Elevated.State.NotSetUp, is Elevated.State.Failed ->
                G7RowAction(label = stringResource(R.string.elevated_set_up), onClick = { setUp() }, accent = true)
            Elevated.State.Waiting -> G7RowAction(
                label = stringResource(R.string.elevated_try_again),
                onClick = {
                    // Picked another way while waiting for this one.
                    val waitingFor =
                        if (Elevated.usesShizuku()) ElevatedMethod.Shizuku else ElevatedMethod.WirelessDebugging
                    if (method != waitingFor) {
                        setUp()
                    } else {
                        scope.launch {
                            if (Elevated.usesShizuku()) {
                                if (!Elevated.start() && !ShizukuElevated.running()) openShizuku(context)
                            } else {
                                PairingNotification.show(context)
                                if (!Elevated.start()) openDeveloperOptionsSettings(context)
                            }
                        }
                    }
                },
            )
            Elevated.State.Running -> G7RowAction(
                label = stringResource(R.string.elevated_turn_off),
                onClick = { scope.launch { Elevated.forget() } },
            )
            Elevated.State.Starting -> CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
        }
    }
    if (state == Elevated.State.Waiting || state is Elevated.State.Failed) {
        if (viaShizuku && shizukuInstalled) {
            G7Row(
                label = stringResource(R.string.elevated_open_shizuku),
                modifier = Modifier.padding(start = 36.dp),
                onClick = { openShizuku(context) },
            )
        } else {
            G7Row(
                label = stringResource(R.string.elevated_open_developer),
                modifier = Modifier.padding(start = 36.dp),
                onClick = {
                    PairingNotification.show(context)
                    openDeveloperOptionsSettings(context)
                },
            )
        }
    }
    // Shizuku and root only show when the phone has them, and nothing asks for
    // either until the user picks it and taps Set up. To change the way it
    // started, turn it off first.
    if (methods.size > 1 && state != Elevated.State.Running && state != Elevated.State.Starting) {
        Column(
            modifier = Modifier.padding(start = 48.dp, end = 12.dp, bottom = 6.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                stringResource(R.string.elevated_start_with),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Choice(
                options = methods.map { option ->
                    option.name to stringResource(
                        when (option) {
                            ElevatedMethod.WirelessDebugging -> R.string.elevated_method_wireless
                            ElevatedMethod.Shizuku -> R.string.elevated_method_shizuku
                            ElevatedMethod.Root -> R.string.elevated_method_root
                        },
                    )
                },
                selected = method.name,
                onSelect = { picked -> method = ElevatedMethod.valueOf(picked) },
            )
            val hint = when (method) {
                ElevatedMethod.WirelessDebugging -> null
                ElevatedMethod.Shizuku -> R.string.elevated_shizuku_hint
                ElevatedMethod.Root -> R.string.elevated_root_hint
            }
            hint?.let {
                Text(
                    stringResource(it),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
    if (steps) {
        AlertDialog(
            onDismissRequest = { steps = false },
            title = { Text(stringResource(R.string.elevated_steps_title)) },
            text = { Text(stringResource(R.string.elevated_steps)) },
            confirmButton = {
                TextButton(onClick = {
                    steps = false
                    PairingNotification.show(context)
                    openDeveloperOptionsSettings(context)
                }) { Text(stringResource(R.string.elevated_open_developer)) }
            },
            dismissButton = {
                TextButton(onClick = { steps = false }) { Text(stringResource(R.string.action_cancel)) }
            },
        )
    }
}

@Composable
private fun RuntimePermissionAccess(
    title: String,
    icon: ImageVector,
    tint: Color,
    granted: Boolean,
    permissions: Array<String>,
    onAccessChanged: () -> Unit,
    highlighted: Boolean = false,
) {
    val context = LocalContext.current
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
        onAccessChanged()
        if (grants.values.none { it }) {
            openAppInfoSettings(context)
        }
    }
    AccessRow(
        label = title,
        icon = icon,
        tint = tint,
        granted = granted,
        onAllow = { ask.launch(permissions) },
        onManage = { openAppInfoSettings(context) },
        highlighted = highlighted,
    )
}

@Composable
private fun PhotosAccess(
    granted: Boolean,
    partial: Boolean,
    onAccessChanged: () -> Unit,
    highlighted: Boolean,
) {
    val context = LocalContext.current
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
        onAccessChanged()
        if (grants.values.none { it }) {
            openAppInfoSettings(context)
        }
    }
    AccessRow(
        label = stringResource(R.string.photos_title),
        icon = G7Icons.photo,
        tint = g7Tints().coral,
        granted = granted && !partial,
        onAllow = { ask.launch(RecentPhotos.permissions) },
        onManage = { openAppInfoSettings(context) },
        highlighted = highlighted,
        allowLabel = stringResource(if (partial) R.string.action_select_photos else R.string.action_allow),
    )
}

@Composable
private fun StorageAccess(
    allFiles: Boolean,
    safFolders: List<String>,
    onAddSafFolder: (Uri) -> Unit,
    onRemoveSafFolder: (String) -> Unit,
    highlighted: Boolean,
) {
    val context = LocalContext.current
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri != null) onAddSafFolder(uri)
    }
    val open = { runCatching { context.startActivity(PhoneStorage.allFilesAccessIntent(context)) }.let {} }
    AccessRow(
        label = stringResource(R.string.storage_all_files_title),
        icon = G7Icons.folder,
        tint = g7Tints().amber,
        granted = allFiles,
        onAllow = open,
        onManage = open,
        highlighted = highlighted,
    )
    if (!allFiles) {
        G7Row(
            label = stringResource(R.string.storage_saf_title),
            modifier = Modifier.padding(start = 36.dp),
        ) {
            G7RowAction(label = stringResource(R.string.storage_add_folder), onClick = { pickFolder.launch(null) })
        }
        safFolders.forEach { name ->
            G7Row(label = name, modifier = Modifier.padding(start = 36.dp)) {
                G7RowAction(label = stringResource(R.string.storage_remove_folder), onClick = { onRemoveSafFolder(name) })
            }
        }
    }
}

/** A setting picked from a few choices: its row, and the choices under it. */
@Composable
private fun ChoiceRow(
    label: String,
    icon: ImageVector,
    tint: Color,
    options: List<Pair<String, String>>,
    selected: String,
    onSelect: (String) -> Unit,
) {
    G7Row(label = label, icon = icon, tint = tint)
    Choice(options, selected, onSelect, Modifier.padding(start = 48.dp, end = 12.dp, bottom = 6.dp))
}

/** The third language choice when it isn't a picked language: opens the list. */
private const val MORE_LANGUAGES = "more"

/** A row of choices, one picked: text tabs in G7, segmented buttons in the other themes. */
@Composable
internal fun Choice(
    options: List<Pair<String, String>>,
    selected: String,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    if (LocalQuietTheme.current) {
        G7Tabs(options = options, selected = selected, onSelect = onSelect, modifier = modifier.fillMaxWidth())
        return
    }
    SingleChoiceSegmentedButtonRow(modifier.fillMaxWidth()) {
        options.forEachIndexed { index, (value, label) ->
            SegmentedButton(
                selected = value == selected,
                onClick = { onSelect(value) },
                shape = SegmentedButtonDefaults.itemShape(index, options.size),
            ) { Text(label) }
        }
    }
}
