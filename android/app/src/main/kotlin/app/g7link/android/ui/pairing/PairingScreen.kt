// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.pairing

import androidx.compose.foundation.border
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.graphics.Color
import app.g7link.android.ui.theme.G7Header
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7PrimaryButton
import app.g7link.android.ui.theme.G7RaisedButton
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.G7RowAction
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.g7Tints
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.g7link.android.R
import app.g7link.android.core.CoreState
import app.g7link.android.core.PairingState
import app.g7link.android.ui.theme.LocalAppFonts
import app.g7link.android.ui.theme.LocalG7Look
import app.g7link.android.ui.theme.LocalReducedMotion
import app.g7link.core.DiscoveredDevice
import app.g7link.core.PairingFailure
import kotlinx.coroutines.delay

/** What the user does on the pairing screen. */
interface PairingActions {
    /** The screen is shown or hidden (discovery needs a multicast lock). */
    fun visible(open: Boolean)
    fun join(link: String)
    fun pairNearby(id: String)
    fun confirm(matches: Boolean)
    fun reset()

    /** A pairing link for another phone or tablet to scan, or null. */
    suspend fun showCode(): String?
}

/**
 * Pairing with a PC: scan the QR code it shows, or pick it from the PCs
 * nearby and compare a 6-digit code. Pairing with another phone or tablet:
 * one shows a code ("Show a code") and the other scans it. `onDone` runs
 * after success (or when the user backs out, if `cancellable`).
 */
@Composable
fun PairingScreen(
    state: CoreState,
    actions: PairingActions,
    cancellable: Boolean,
    onDone: () -> Unit,
) {
    var showing by rememberSaveable { mutableStateOf(false) }
    // A scanned code, until the user says to pair with it.
    var scanned by rememberSaveable { mutableStateOf<String?>(null) }
    var declined by rememberSaveable { mutableStateOf<String?>(null) }
    val pairing = state.pairing
    scanned?.let { link ->
        PairingLinkDialog(
            link = link,
            fromOtherApp = false,
            onConfirm = {
                scanned = null
                actions.join(link)
            },
            onDismiss = {
                scanned = null
                declined = link
            },
        )
    }
    val reducedMotion = LocalReducedMotion.current
    DisposableEffect(Unit) {
        actions.visible(true)
        onDispose { actions.visible(false) }
    }

    LaunchedEffect(pairing) {
        if (pairing is PairingState.Paired) {
            delay(1_400)
            actions.reset()
            onDone()
        }
    }

    val tints = g7Tints()
    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        G7Header(
            title = when (pairing) {
                is PairingState.Comparing, is PairingState.Confirmed -> stringResource(R.string.pair_check_code)
                is PairingState.Paired -> stringResource(R.string.pair_done_title)
                is PairingState.Failed -> stringResource(R.string.pair_failed_title)
                else -> if (showing) stringResource(R.string.pair_show_title) else stringResource(R.string.action_pair_with_pc)
            },
            onBack = if (cancellable && pairing !is PairingState.Paired) {
                {
                    actions.reset()
                    onDone()
                }
            } else {
                null
            },
        )

        AnimatedContent(
            targetState = pairing to showing,
            transitionSpec = {
                if (reducedMotion) {
                    EnterTransition.None togetherWith ExitTransition.None
                } else {
                    fadeIn() togetherWith fadeOut()
                }
            },
            modifier = Modifier.weight(1f).fillMaxWidth(),
            label = "pairing",
        ) { (current, showCode) ->
            when (current) {
                PairingState.Idle -> if (showCode) {
                    Column(Modifier.fillMaxSize().padding(horizontal = 24.dp, vertical = 12.dp)) {
                        ShowCode(actions)
                    }
                } else {
                    Column(
                        Modifier
                            .fillMaxSize()
                            .verticalScroll(rememberScrollState())
                            .padding(horizontal = 10.dp, vertical = 6.dp),
                    ) {
                        // The camera keeps finding a code it was told no to: asked once.
                        Scan { link -> if (link != declined) scanned = link }
                        NearbyList(state.discovered, actions::pairNearby)
                    }
                }
                else -> Column(
                    Modifier.fillMaxSize().padding(horizontal = 24.dp, vertical = 24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    when (current) {
                        PairingState.Joining -> Working(stringResource(R.string.pair_joining))
                        is PairingState.Connecting ->
                            Working(stringResource(R.string.pair_connecting, state.nameOf(current.peer) ?: ""))
                        is PairingState.Comparing -> Compare(state.nameOf(current.peer), current.code, actions::confirm)
                        is PairingState.Confirmed -> {
                            CodeDigits(current.code)
                            Spacer(Modifier.height(24.dp))
                            Working(stringResource(R.string.pair_waiting, state.nameOf(current.peer) ?: ""))
                        }
                        is PairingState.Paired -> Outcome(true, stringResource(R.string.pair_done, current.name))
                        is PairingState.Failed -> {
                            Outcome(false, failureText(current.failure))
                            Spacer(Modifier.height(20.dp))
                            G7PrimaryButton(label = stringResource(R.string.action_try_again), onClick = actions::reset)
                        }
                        PairingState.Idle -> Unit
                    }
                }
            }
        }

        if (pairing == PairingState.Idle) {
            Column(Modifier.padding(start = 10.dp, end = 10.dp, bottom = 16.dp)) {
                if (!showing) {
                    Text(
                        stringResource(R.string.pair_none_showing),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
                    )
                }
                G7Row(
                    label = stringResource(if (showing) R.string.action_scan_instead else R.string.action_show_code),
                    icon = if (showing) G7Icons.camera else G7Icons.phone,
                    tint = tints.muted,
                    onClick = { showing = !showing },
                )
            }
        }
    }
}

/** This device's pairing code, for another phone or tablet to scan. */
@Composable
private fun ShowCode(actions: PairingActions) {
    var link by androidx.compose.runtime.remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { link = actions.showCode() }
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(
            stringResource(R.string.pair_show_hint),
            style = MaterialTheme.typography.bodyLarge,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(20.dp))
        val current = link
        val label = stringResource(R.string.pair_show_qr_desc)
        Box(
            modifier = Modifier
                .fillMaxWidth(0.8f)
                .aspectRatio(1f)
                .clip(androidx.compose.foundation.shape.RoundedCornerShape(20.dp))
                .background(androidx.compose.ui.graphics.Color.White)
                .padding(16.dp)
                .semantics { contentDescription = label },
            contentAlignment = Alignment.Center,
        ) {
            if (current == null) {
                CircularProgressIndicator()
            } else {
                QrCode(current)
            }
        }
    }
}

/** [text] as a QR code, black on the white it sits on. */
@Composable
private fun QrCode(text: String) {
    val matrix = androidx.compose.runtime.remember(text) {
        runCatching {
            com.google.zxing.qrcode.QRCodeWriter().encode(
                text,
                com.google.zxing.BarcodeFormat.QR_CODE,
                0,
                0,
                mapOf(com.google.zxing.EncodeHintType.MARGIN to 0),
            )
        }.getOrNull()
    } ?: return
    androidx.compose.foundation.Canvas(Modifier.fillMaxSize()) {
        val cell = size.minDimension / matrix.width
        for (y in 0 until matrix.height) {
            for (x in 0 until matrix.width) {
                if (matrix.get(x, y)) {
                    drawRect(
                        androidx.compose.ui.graphics.Color.Black,
                        topLeft = androidx.compose.ui.geometry.Offset(x * cell, y * cell),
                        size = androidx.compose.ui.geometry.Size(cell, cell),
                    )
                }
            }
        }
    }
}

@Composable
private fun Scan(onLink: (String) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Box(
        modifier = Modifier
            .padding(start = 10.dp, end = 10.dp, top = 4.dp, bottom = 12.dp)
            .fillMaxWidth()
            .aspectRatio(1f)
            .clip(androidx.compose.foundation.shape.RoundedCornerShape(14.dp))
            .background(LocalG7Look.current.deepGround),
        contentAlignment = Alignment.Center,
    ) {
        QrScanner(onLink = onLink, modifier = Modifier.fillMaxSize())
        Box(
            Modifier
                .fillMaxWidth(0.58f)
                .aspectRatio(1f)
                .border(2.dp, scheme.primary, androidx.compose.foundation.shape.RoundedCornerShape(12.dp)),
        )
        Text(
            stringResource(R.string.pair_point_at),
            style = MaterialTheme.typography.labelMedium,
            color = Color(0xFFA1A1A8),
            textAlign = TextAlign.Center,
            modifier = Modifier.align(Alignment.BottomCenter).padding(horizontal = 16.dp, vertical = 14.dp),
        )
    }
}

@Composable
private fun NearbyList(devices: List<DiscoveredDevice>, onPair: (String) -> Unit) {
    val reducedMotion = LocalReducedMotion.current
    val tints = g7Tints()
    Row(
        modifier = Modifier.padding(start = 12.dp, end = 12.dp, top = 8.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            stringResource(R.string.pair_nearby_label),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        G7StatusDot(tints.teal)
    }
    if (devices.isEmpty()) {
        G7Row(label = stringResource(R.string.pair_looking), icon = G7Icons.monitor, tint = tints.muted) {
            if (reducedMotion) {
                CircularProgressIndicator(progress = { 0.75f }, modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
            } else {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            }
        }
    }
    devices.forEach { device ->
        G7Row(
            label = device.name ?: stringResource(R.string.your_pc),
            icon = G7Icons.monitor,
            tint = tints.lavender,
            onClick = { onPair(device.id) },
        ) {
            G7RowAction(label = stringResource(R.string.action_pair), onClick = { onPair(device.id) }, accent = true)
        }
    }
}

@Composable
private fun Compare(peerName: String?, code: String, onConfirm: (Boolean) -> Unit) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(
            stringResource(R.string.pair_compare_hint, peerName ?: stringResource(R.string.your_pc)),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(24.dp))
        CodeDigits(code)
        Spacer(Modifier.height(32.dp))
        G7PrimaryButton(label = stringResource(R.string.action_codes_match), onClick = { onConfirm(true) })
        Spacer(Modifier.height(8.dp))
        G7RaisedButton(
            label = stringResource(R.string.action_codes_differ),
            onClick = { onConfirm(false) },
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun CodeDigits(code: String) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.semantics { contentDescription = code.toList().joinToString(" ") },
    ) {
        code.forEach { digit ->
            Surface(
                shape = MaterialTheme.shapes.medium,
                color = MaterialTheme.colorScheme.surfaceContainer,
                modifier = Modifier.size(width = 44.dp, height = 56.dp),
            ) {
                Box(contentAlignment = Alignment.Center) {
                    Text(digit.toString(), fontFamily = LocalAppFonts.current.mono, fontSize = 26.sp, fontWeight = FontWeight.Medium)
                }
            }
        }
    }
}

@Composable
private fun Working(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        if (LocalReducedMotion.current) {
            CircularProgressIndicator(
                progress = { 0.75f },
                modifier = Modifier.size(20.dp),
                strokeWidth = 2.dp,
            )
        } else {
            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
        }
        Spacer(Modifier.width(12.dp))
        Text(text, style = MaterialTheme.typography.bodyLarge)
    }
}

@Composable
private fun Outcome(success: Boolean, text: String) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Surface(
            shape = CircleShape,
            color = MaterialTheme.colorScheme.surfaceContainer,
            modifier = Modifier.size(64.dp),
        ) {
            Box(contentAlignment = Alignment.Center) {
                Text(
                    if (success) "✓" else "!",
                    fontSize = 28.sp,
                    fontWeight = FontWeight.Bold,
                    color = if (success) g7Tints().teal else g7Tints().red,
                )
            }
        }
        Spacer(Modifier.height(16.dp))
        Text(text, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
    }
}

@Composable
private fun failureText(failure: PairingFailure): String = when (failure) {
    PairingFailure.Rejected -> stringResource(R.string.pair_failed_rejected)
    PairingFailure.Declined -> stringResource(R.string.pair_failed_declined)
    PairingFailure.Expired -> stringResource(R.string.pair_failed_expired)
    PairingFailure.Unreachable -> stringResource(R.string.pair_failed_unreachable)
    is PairingFailure.Other -> stringResource(R.string.pair_failed_other)
}
