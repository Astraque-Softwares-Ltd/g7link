// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.webcam

import androidx.compose.foundation.clickable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import app.g7link.android.ui.theme.G7Colors
import app.g7link.android.ui.theme.G7IconButton
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.G7Tabs
import app.g7link.android.ui.theme.LocalQuietTheme
import app.g7link.android.ui.theme.g7Tints
import android.Manifest
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Device
import app.g7link.android.ui.detached
import app.g7link.android.webcam.WebcamService

@Composable
fun WebcamScreen(
    device: Device,
    autoStart: Boolean = false,
    initialHeight: Int? = null,
    initialCamera: String? = null,
    onAutoStartConsumed: () -> Unit = {},
    onAccessChanged: () -> Unit = {},
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val view = LocalView.current
    val session by WebcamService.session.collectAsStateWithLifecycle()

    // Keep the phone's screen on while the webcam viewfinder/controls are open.
    DisposableEffect(view) {
        val prev = view.keepScreenOn
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = prev }
    }

    var hasCameraPermission by remember { mutableStateOf(WebcamService.hasPermission(context)) }
    var startAfterGrant by remember { mutableStateOf(false) }
    val askCamera = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        hasCameraPermission = WebcamService.hasPermission(context)
        WebcamService.refreshCapabilities(context)
        onAccessChanged()
        if (granted && startAfterGrant) {
            startAfterGrant = false
            WebcamService.start(
                context = context,
                pcId = device.id,
                height = initialHeight ?: session.height,
                camera = initialCamera ?: session.camera,
            )
        } else {
            startAfterGrant = false
        }
    }

    LaunchedEffect(Unit) {
        WebcamService.refreshCapabilities(context)
    }

    LaunchedEffect(initialHeight, initialCamera) {
        if (initialHeight != null) WebcamService.setResolutionHeight(initialHeight)
        if (initialCamera != null) WebcamService.setCamera(initialCamera, context)
    }

    LaunchedEffect(autoStart, hasCameraPermission, device.online) {
        if (autoStart) {
            onAutoStartConsumed()
            if (hasCameraPermission && device.online) {
                WebcamService.start(
                    context = context,
                    pcId = device.id,
                    height = initialHeight ?: session.height,
                    camera = initialCamera ?: session.camera,
                )
            } else if (!hasCameraPermission) {
                startAfterGrant = true
                askCamera.launch(Manifest.permission.CAMERA)
            }
        }
    }

    val previewView = remember { WebcamPreviewView(context, WebcamService::attachUiPreview) }
    val previewShape = RoundedCornerShape(16.dp)
    val previewDensity = LocalDensity.current
    val previewDirection = LocalLayoutDirection.current
    LifecycleStartEffect(previewView) {
        previewView.setPreviewVisible(true)
        onStopOrDispose { previewView.setPreviewVisible(false) }
    }

    val isStreamingHere = session.active && session.pcId == device.id

    val tints = g7Tints()
    val scheme = MaterialTheme.colorScheme
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(if (LocalQuietTheme.current) G7Colors.deepGround else scheme.background),
    ) {
        // Back, whether it's live, and what it streams
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 4.dp, end = 16.dp, top = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            G7IconButton(icon = G7Icons.back, contentDescription = stringResource(R.string.action_back), onClick = onBack)
            val live = isStreamingHere && !session.connecting
            G7StatusDot(if (live) tints.red else if (device.online) tints.teal else tints.muted)
            Text(
                text = when {
                    live -> stringResource(R.string.webcam_live_on, device.name)
                    isStreamingHere -> stringResource(R.string.webcam_status_connecting, device.name)
                    else -> device.name
                },
                style = MaterialTheme.typography.labelMedium,
                color = if (live) tints.red else scheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(
                text = stringResource(R.string.webcam_stream_spec, session.width, session.height, session.fps),
                style = MaterialTheme.typography.labelMedium,
                color = scheme.onSurfaceVariant,
                maxLines = 1,
            )
        }

        if (!hasCameraPermission) {
            G7Row(
                label = stringResource(R.string.webcam_permission_title),
                icon = G7Icons.camera,
                tint = tints.coral,
                modifier = Modifier.padding(horizontal = 4.dp),
            ) {
                G7RowAction(
                    label = stringResource(R.string.action_allow),
                    onClick = {
                        startAfterGrant = device.online
                        askCamera.launch(Manifest.permission.CAMERA)
                    },
                    accent = true,
                )
            }
        }
        if (!device.online) {
            G7Row(
                label = stringResource(R.string.not_connected),
                sublabel = stringResource(R.string.webcam_offline_hint, device.name),
                icon = G7Icons.wifi,
                tint = tints.muted,
                modifier = Modifier.padding(horizontal = 4.dp),
            )
        }

        // The camera's view
        Box(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 4.dp)
                .clip(RoundedCornerShape(16.dp))
                .background(Color(0xFF1D1D21)),
            contentAlignment = Alignment.Center,
        ) {
            if (isStreamingHere && hasCameraPermission) {
                AndroidView(
                    factory = { previewView.detached() },
                    update = { it.setPreviewShape(previewShape, previewDensity, previewDirection) },
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                Text(
                    text = if (isStreamingHere && session.connecting) {
                        stringResource(R.string.webcam_status_connecting, device.name)
                    } else {
                        stringResource(R.string.webcam_status_ready, device.name)
                    },
                    style = MaterialTheme.typography.bodyMedium,
                    color = Color(0xFFA1A1A8),
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(24.dp),
                )
            }
        }

        // Resolution, then zoom: presets and a slider
        val res720p = stringResource(R.string.webcam_res_720p)
        val res1080p = stringResource(R.string.webcam_res_1080p)
        val res4k = stringResource(R.string.webcam_res_4k)
        val resolutions = buildList {
            add(720 to res720p)
            add(1080 to res1080p)
            if (session.supports4k) add(2160 to res4k)
        }
        Row(Modifier.fillMaxWidth().padding(top = 10.dp), horizontalArrangement = Arrangement.Center) {
            G7Tabs(options = resolutions, selected = session.resolution, onSelect = { WebcamService.setResolutionHeight(it) })
        }
        val zoomPresets = buildList {
            add(1f)
            if (session.maxZoomRatio >= 2f) add(2f)
            if (session.maxZoomRatio >= 4f) add(4f)
        }
        Row(
            modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
        ) {
            zoomPresets.forEach { ratio ->
                val selected = kotlin.math.abs(session.zoomRatio - ratio) < 0.08f
                Text(
                    stringResource(R.string.webcam_zoom_preset, ratio.toInt()),
                    style = MaterialTheme.typography.labelMedium,
                    color = if (selected) scheme.onPrimary else scheme.onSurfaceVariant,
                    modifier = Modifier
                        .clip(RoundedCornerShape(16.dp))
                        .background(if (selected) scheme.primary else scheme.surface)
                        .clickable(role = Role.Button) { WebcamService.setZoomRatio(ratio.coerceAtMost(session.maxZoomRatio)) }
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                )
            }
        }
        val zoomLabel = stringResource(R.string.webcam_zoom_label, session.zoomRatio)
        val minZ = session.minZoomRatio
        val maxZ = session.maxZoomRatio.coerceAtLeast(minZ + 0.1f)
        Slider(
            value = session.zoomRatio.coerceIn(minZ, maxZ),
            onValueChange = { WebcamService.setZoomRatio(it) },
            valueRange = minZ..maxZ,
            colors = SliderDefaults.colors(
                thumbColor = scheme.primary,
                activeTrackColor = scheme.primary,
                inactiveTrackColor = scheme.surfaceContainerHigh,
            ),
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 32.dp)
                .semantics { contentDescription = zoomLabel },
        )

        // Switch camera, start or stop, and the torch when there's one
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, top = 4.dp, bottom = 20.dp),
            horizontalArrangement = Arrangement.SpaceAround,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RoundButton(
                icon = G7Icons.refresh,
                label = stringResource(
                    if (session.camera == "front") R.string.webcam_switch_to_back else R.string.webcam_switch_to_front,
                ),
                size = 52.dp,
                background = scheme.surface,
                tint = scheme.onSurface,
                onClick = {
                    val next = if (session.camera == "front") "back" else "front"
                    WebcamService.setCamera(next, context)
                },
            )
            if (isStreamingHere) {
                RoundButton(
                    icon = G7Icons.stop,
                    label = stringResource(R.string.webcam_stop),
                    size = 64.dp,
                    background = tints.red,
                    tint = Color(0xFF2A1111),
                    onClick = { WebcamService.stop(context) },
                )
            } else {
                RoundButton(
                    icon = G7Icons.camera,
                    label = stringResource(R.string.webcam_start),
                    size = 64.dp,
                    background = scheme.primary,
                    tint = scheme.onPrimary,
                    enabled = device.online,
                    onClick = {
                        if (!hasCameraPermission) {
                            startAfterGrant = true
                            askCamera.launch(Manifest.permission.CAMERA)
                        } else {
                            WebcamService.start(
                                context = context,
                                pcId = device.id,
                                height = session.height,
                                camera = session.camera,
                            )
                        }
                    },
                )
            }
            if (session.hasFlash) {
                RoundButton(
                    icon = G7Icons.torch,
                    label = stringResource(if (session.torchOn) R.string.webcam_turn_torch_off else R.string.webcam_turn_torch_on),
                    size = 52.dp,
                    background = if (session.torchOn) tints.amber else scheme.surface,
                    tint = if (session.torchOn) Color(0xFF2A1A00) else scheme.onSurface,
                    enabled = isStreamingHere,
                    onClick = { WebcamService.setTorch(!session.torchOn) },
                )
            } else {
                Spacer(Modifier.size(52.dp))
            }
        }
    }
}

/** A round camera control with an icon; its label is for screen readers. */
@Composable
private fun RoundButton(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    size: androidx.compose.ui.unit.Dp,
    background: Color,
    tint: Color,
    onClick: () -> Unit,
    enabled: Boolean = true,
) {
    Box(
        modifier = Modifier
            .size(size)
            .clip(CircleShape)
            .background(background)
            .clickable(enabled = enabled, role = Role.Button, onClickLabel = label, onClick = onClick)
            .semantics { contentDescription = label }
            .alpha(if (enabled) 1f else 0.45f),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(22.dp))
    }
}

