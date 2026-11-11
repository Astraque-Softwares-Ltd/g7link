// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.guide

import androidx.compose.foundation.background
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Icon
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7Tabs
import app.g7link.android.ui.theme.G7Tints
import app.g7link.android.ui.theme.LocalQuietTheme
import app.g7link.android.ui.theme.g7Tints
import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import app.g7link.android.R
import app.g7link.android.calls.PhoneCalls
import app.g7link.android.contacts.PhoneContacts
import app.g7link.android.core.BackgroundAccess
import app.g7link.android.core.CoreState
import app.g7link.android.elevated.Elevated
import app.g7link.android.elevated.PairingNotification
import app.g7link.android.mirror.InputService
import app.g7link.android.notifications.NotificationListener
import app.g7link.android.photos.RecentPhotos
import app.g7link.android.sms.PhoneSms
import app.g7link.android.storage.PhoneStorage
import app.g7link.android.toggles.PhoneToggles
import app.g7link.android.ui.theme.LocalGlassTheme
import app.g7link.android.ui.theme.bentoWell
import app.g7link.android.ui.theme.glassModalBlur
import app.g7link.android.ui.theme.glassModalContainerColor
import app.g7link.android.ui.theme.liquidGlassCard
import kotlinx.coroutines.launch

/**
 * Interactive Features Guide screen with search, area filters, live capability badges,
 * "Show me" navigation, and detailed "Learn more" user guide sheets.
 */
@androidx.compose.runtime.Immutable
private data class ResolvedGuideCard(
    val id: String,
    val title: String,
    val summary: String,
    val whereLine: String,
    val needsLine: String,
    val externalSteps: List<String>,
    val showMeTarget: GuideHighlightTarget?,
    val badge: FeatureLiveBadge?,
    val badgeText: String?,
    val fixAction: GuideFixAction?,
    val fixButtonLabel: String?,
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FeaturesGuideScreen(
    state: CoreState,
    onShowMe: (GuideHighlightTarget) -> Unit,
    onOpenWalkthrough: () -> Unit,
    onPairNew: () -> Unit,
    onAccessChanged: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val isGlass = LocalGlassTheme.current
    var searchQuery by rememberSaveable { mutableStateOf("") }
    var selectedAreaId by rememberSaveable { mutableStateOf(FeatureArea.All.id) }
    var activeLearnMoreId by rememberSaveable { mutableStateOf<String?>(null) }

    val hasConnectedPc = state.devices.any { it.online }

    val selectedArea = remember(selectedAreaId) {
        FeatureArea.entries.firstOrNull { it.id == selectedAreaId } ?: FeatureArea.All
    }
    val filteredEntries = remember(context, selectedArea, searchQuery) {
        FeaturesGuideCatalog.filter(context, selectedArea, searchQuery)
    }
    val resolvedCards = remember(context, filteredEntries, state, hasConnectedPc) {
        val res = context.resources
        filteredEntries.map { entry ->
            val badge = entry.evaluateBadge(state)
            val showBadge = hasConnectedPc ||
                badge is FeatureLiveBadge.NeedsPermission ||
                badge is FeatureLiveBadge.NeedsPowerLevel
            val badgeText = if (showBadge) {
                when (badge) {
                    is FeatureLiveBadge.Available -> res.getString(badge.labelRes)
                    is FeatureLiveBadge.NeedsPermission ->
                        res.getString(badge.labelRes, res.getString(badge.permissionNameRes))
                    is FeatureLiveBadge.NeedsPowerLevel ->
                        res.getString(badge.labelRes, res.getString(badge.levelNameRes))
                    is FeatureLiveBadge.ConnectPcToCheck -> res.getString(badge.labelRes)
                }
            } else {
                null
            }
            val fixAction = entry.evaluateFixAction(state)
            ResolvedGuideCard(
                id = entry.id,
                title = res.getString(entry.titleRes),
                summary = res.getString(entry.summaryRes),
                whereLine = res.getString(R.string.guide_where_prefix, res.getString(entry.whereTextRes)),
                needsLine = res.getString(R.string.guide_needs_prefix, res.getString(entry.needsSummaryRes)),
                externalSteps = entry.externalStepResIds.map { res.getString(it) },
                showMeTarget = entry.showMeTarget,
                badge = if (showBadge) badge else null,
                badgeText = badgeText,
                fixAction = fixAction,
                fixButtonLabel = fixAction?.let { res.getString(it.buttonLabelRes) },
            )
        }
    }
    val activeLearnMoreEntry = remember(activeLearnMoreId) {
        activeLearnMoreId?.let { id -> FeaturesGuideCatalog.entries.firstOrNull { it.id == id } }
    }

    val callsLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { onAccessChanged() }

    val smsContactsLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { onAccessChanged() }

    val photosLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { onAccessChanged() }

    val runFixAction: (GuideFixAction) -> Unit = remember(context, scope, onAccessChanged, onPairNew) {
        { action ->
            when (action) {
                GuideFixAction.GRANT_NOTIFICATIONS -> {
                    runCatching { context.startActivity(NotificationListener.settingsIntent(context)) }
                }
                GuideFixAction.GRANT_BATTERY -> {
                    runCatching { context.startActivity(BackgroundAccess.requestIntent(context)) }
                }
                GuideFixAction.GRANT_CALLS -> {
                    callsLauncher.launch(PhoneCalls.permissions)
                }
                GuideFixAction.GRANT_SMS_CONTACTS -> {
                    smsContactsLauncher.launch(PhoneSms.permissions + PhoneContacts.permissions)
                }
                GuideFixAction.GRANT_PHOTOS -> {
                    photosLauncher.launch(RecentPhotos.permissions)
                }
                GuideFixAction.GRANT_STORAGE -> {
                    runCatching { context.startActivity(PhoneStorage.allFilesAccessIntent(context)) }
                }
                GuideFixAction.OPEN_ACCESSIBILITY -> {
                    runCatching { context.startActivity(InputService.settingsIntent()) }
                }
                GuideFixAction.ENABLE_ELEVATED -> {
                    scope.launch {
                        PairingNotification.show(context)
                        if (!Elevated.start()) {
                            openDevOptions(context)
                        }
                        onAccessChanged()
                    }
                }
                GuideFixAction.ENABLE_DND -> {
                    runCatching { context.startActivity(PhoneToggles.dndSettingsIntent()) }
                }
                GuideFixAction.ENABLE_WRITE_SETTINGS -> {
                    runCatching { context.startActivity(PhoneToggles.writeSettingsIntent(context)) }
                }
                GuideFixAction.PAIR_PC -> {
                    onPairNew()
                }
            }
        }
    }

    val searchDesc = stringResource(R.string.guide_search_desc)

    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    LazyColumn(
        modifier = modifier
            .fillMaxSize()
            .glassModalBlur(isGlass && activeLearnMoreEntry != null),
        contentPadding = PaddingValues(start = 10.dp, top = 8.dp, end = 10.dp, bottom = if (LocalQuietTheme.current) 16.dp else 104.dp),
    ) {
        item(key = "guide_header", contentType = "guide_header") {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(horizontal = 6.dp)) {
                Text(
                    text = stringResource(R.string.home_guide_title),
                    style = MaterialTheme.typography.titleLarge,
                    modifier = Modifier.padding(start = 6.dp, top = 12.dp),
                )
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .background(scheme.surfaceContainer)
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(
                        imageVector = G7Icons.search,
                        contentDescription = null,
                        tint = scheme.onSurfaceVariant,
                        modifier = Modifier.size(18.dp),
                    )
                    Box(Modifier.weight(1f)) {
                        if (searchQuery.isEmpty()) {
                            Text(
                                stringResource(R.string.guide_search_placeholder),
                                style = MaterialTheme.typography.bodyMedium,
                                color = scheme.onSurfaceVariant,
                                maxLines = 1,
                            )
                        }
                        BasicTextField(
                            value = searchQuery,
                            onValueChange = { searchQuery = it },
                            singleLine = true,
                            textStyle = MaterialTheme.typography.bodyMedium.copy(color = scheme.onSurface),
                            cursorBrush = SolidColor(scheme.primary),
                            modifier = Modifier
                                .fillMaxWidth()
                                .semantics { contentDescription = searchDesc },
                        )
                    }
                }
                G7Tabs(
                    options = FeatureArea.entries.map { it.id to stringResource(it.labelRes) },
                    selected = selectedArea.id,
                    onSelect = { selectedAreaId = it },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }

        if (state.devices.isEmpty()) {
            item(key = "guide_pair", contentType = "row") {
                G7Row(
                    label = stringResource(R.string.action_pair_with_pc),
                    icon = G7Icons.plus,
                    tint = tints.lavender,
                    onClick = onPairNew,
                )
            }
        }

        if (resolvedCards.isEmpty()) {
            item(key = "guide_empty", contentType = "row") {
                G7Row(
                    label = stringResource(R.string.guide_empty_title),
                    sublabel = stringResource(R.string.guide_empty_body),
                    icon = G7Icons.search,
                    tint = tints.muted,
                )
            }
        } else {
            items(items = resolvedCards, key = { it.id }, contentType = { "row" }) { card ->
                val (icon, tint) = guideIcon(card.id, tints)
                G7Row(
                    label = card.title,
                    icon = icon,
                    tint = tint,
                    onClick = { activeLearnMoreId = card.id },
                ) {
                    val badge = card.badge
                    if (badge != null) {
                        Text(
                            text = when (badge) {
                                is FeatureLiveBadge.Available -> stringResource(R.string.guide_state_ready)
                                is FeatureLiveBadge.NeedsPermission -> stringResource(R.string.action_allow)
                                is FeatureLiveBadge.NeedsPowerLevel -> stringResource(badge.levelNameRes)
                                is FeatureLiveBadge.ConnectPcToCheck -> stringResource(R.string.guide_state_off)
                            },
                            style = MaterialTheme.typography.bodyMedium,
                            color = when (badge) {
                                is FeatureLiveBadge.Available -> tints.teal
                                is FeatureLiveBadge.NeedsPermission,
                                is FeatureLiveBadge.NeedsPowerLevel,
                                -> tints.amber
                                is FeatureLiveBadge.ConnectPcToCheck -> scheme.onSurfaceVariant
                            },
                            maxLines = 1,
                        )
                    }
                }
            }
        }

        item(key = "guide_walkthrough", contentType = "row") {
            G7Row(
                label = stringResource(R.string.guide_open_walkthrough),
                icon = G7Icons.info,
                tint = tints.muted,
                onClick = onOpenWalkthrough,
            )
        }
    }

    if (activeLearnMoreEntry != null) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { activeLearnMoreId = null },
            sheetState = sheetState,
            containerColor = glassModalContainerColor(),
        ) {
            FeatureLearnMoreSheetContent(
                entry = activeLearnMoreEntry,
                state = state,
                hasConnectedPc = hasConnectedPc,
                onShowMe = { target ->
                    activeLearnMoreId = null
                    onShowMe(target)
                },
                onRunFix = runFixAction,
                onClose = { activeLearnMoreId = null },
            )
        }
    }
}

@Composable
private fun FeatureBadgeChip(
    badge: FeatureLiveBadge,
    text: String? = null,
) {
    val (bg, fg) = when (badge) {
        is FeatureLiveBadge.Available ->
            MaterialTheme.colorScheme.primary.copy(alpha = 0.16f) to MaterialTheme.colorScheme.primary
        is FeatureLiveBadge.NeedsPermission,
        is FeatureLiveBadge.NeedsPowerLevel,
        -> MaterialTheme.colorScheme.tertiaryContainer to MaterialTheme.colorScheme.onTertiaryContainer
        is FeatureLiveBadge.ConnectPcToCheck ->
            MaterialTheme.colorScheme.surfaceContainerHighest to MaterialTheme.colorScheme.onSurfaceVariant
    }
    val resolvedText = text ?: when (badge) {
        is FeatureLiveBadge.Available -> stringResource(badge.labelRes)
        is FeatureLiveBadge.NeedsPermission ->
            stringResource(badge.labelRes, stringResource(badge.permissionNameRes))
        is FeatureLiveBadge.NeedsPowerLevel ->
            stringResource(badge.labelRes, stringResource(badge.levelNameRes))
        is FeatureLiveBadge.ConnectPcToCheck -> stringResource(badge.labelRes)
    }
    Text(
        text = resolvedText,
        style = MaterialTheme.typography.labelSmall,
        color = fg,
        modifier = Modifier
            .bentoWell(cornerRadius = 10.dp, fillColor = bg)
            .padding(horizontal = 8.dp, vertical = 4.dp),
    )
}

@Composable
private fun FeatureLearnMoreSheetContent(
    entry: FeatureGuideEntry,
    state: CoreState,
    hasConnectedPc: Boolean,
    onShowMe: (GuideHighlightTarget) -> Unit,
    onRunFix: (GuideFixAction) -> Unit,
    onClose: () -> Unit,
) {
    val badge = entry.evaluateBadge(state)
    val fixAction = entry.evaluateFixAction(state)
    val showBadge = hasConnectedPc ||
        badge is FeatureLiveBadge.NeedsPermission ||
        badge is FeatureLiveBadge.NeedsPowerLevel

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState())
            .padding(start = 22.dp, end = 22.dp, bottom = 32.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text(
            text = stringResource(entry.area.labelRes),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.primary,
        )
        Text(
            text = stringResource(entry.titleRes),
            style = MaterialTheme.typography.headlineSmall,
        )
        if (showBadge) {
            FeatureBadgeChip(badge = badge)
        }
        Text(
            text = stringResource(entry.summaryRes),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_where),
            items = listOf(stringResource(entry.whereTextRes)),
        )

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_needs),
            items = entry.whatYouNeedResIds.map { stringResource(it) },
        )

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_steps_phone),
            items = entry.stepByStepPhoneResIds.mapIndexed { idx, resId ->
                stringResource(R.string.guide_numbered_step, idx + 1, stringResource(resId))
            },
        )

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_steps_pc),
            items = entry.stepByStepPcResIds.mapIndexed { idx, resId ->
                stringResource(R.string.guide_numbered_step, idx + 1, stringResource(resId))
            },
        )

        if (entry.externalStepResIds.isNotEmpty()) {
            GuideSectionBlock(
                heading = stringResource(R.string.guide_external_heading),
                items = entry.externalStepResIds.map { stringResource(it) },
            )
        }

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_options),
            items = entry.optionsExplainedResIds.map { stringResource(it) },
        )

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_privacy),
            items = listOf(stringResource(entry.dataAndPrivacyRes)),
        )

        GuideSectionBlock(
            heading = stringResource(R.string.guide_sheet_limits),
            items = listOf(stringResource(entry.limitsRes)),
        )

        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.35f))

        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (entry.showMeTarget != null) {
                FilledTonalButton(onClick = { onShowMe(entry.showMeTarget) }) {
                    Text(stringResource(R.string.guide_action_show_me))
                }
            }
            if (fixAction != null) {
                Button(onClick = { onRunFix(fixAction) }) {
                    Text(stringResource(fixAction.buttonLabelRes))
                }
            }
            Spacer(Modifier.weight(1f))
            OutlinedButton(onClick = onClose) {
                Text(stringResource(R.string.guide_action_done))
            }
        }
    }
}

@Composable
private fun GuideSectionBlock(
    heading: String,
    items: List<String>,
) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            text = heading,
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        items.forEach { line ->
            Text(
                text = line,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

private fun openDevOptions(context: Context) {
    val devIntent = Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    if (runCatching { context.startActivity(devIntent) }.isFailure) {
        runCatching {
            context.startActivity(Intent(Settings.ACTION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }
}

/** Each feature's icon and tint, as Home draws it. */
private fun guideIcon(id: String, tints: G7Tints): Pair<ImageVector, Color> = when (id) {
    "notification_mirroring" -> G7Icons.bell to tints.blue
    "incoming_calls" -> G7Icons.call to tints.teal
    "media_and_pc_audio" -> G7Icons.music to tints.pink
    "sms_mms_messages" -> G7Icons.message to tints.blue
    "clipboard_sync", "quick_settings_clipboard_tile" -> G7Icons.clipboard to tints.lavender
    "send_files_and_folders" -> G7Icons.upload to tints.amber
    "waiting_to_send_queue" -> G7Icons.history to tints.amber
    "share_sheet_handoff" -> G7Icons.send to tints.blue
    "browse_phone_storage" -> G7Icons.folder to tints.amber
    "browse_pc_files" -> G7Icons.folder to tints.amber
    "recent_photos" -> G7Icons.photo to tints.coral
    "timeline_history" -> G7Icons.history to tints.blue
    "localsend_lan" -> G7Icons.wifi to tints.green
    "phone_screen_mirroring" -> G7Icons.screen to tints.pink
    "phone_app_windows" -> G7Icons.monitor to tints.pink
    "phone_as_pc_webcam", "continuity_camera" -> G7Icons.camera to tints.coral
    "voice_recorder" -> G7Icons.mic to tints.red
    "control_pc_remote" -> G7Icons.pointer to tints.teal
    "control_phone_from_pc" -> G7Icons.gesture to tints.pink
    "elevated_mode" -> G7Icons.bolt to tints.amber
    "deck_control_surface" -> G7Icons.deck to tints.green
    "desk_mode" -> G7Icons.clock to tints.lavender
    "ring_lock_sleep_wake" -> G7Icons.power to tints.red
    "phone_quick_toggles" -> G7Icons.moon to tints.lavender
    "away_mode_and_usb" -> G7Icons.signal to tints.teal
    "connection_doctor" -> G7Icons.info to tints.blue
    else -> G7Icons.info to tints.muted
}
