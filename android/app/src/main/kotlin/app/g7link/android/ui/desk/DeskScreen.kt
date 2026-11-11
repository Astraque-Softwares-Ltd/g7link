// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.desk

import app.g7link.android.ui.theme.G7Slider
import androidx.compose.foundation.clickable
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import app.g7link.android.ui.home.linkText
import app.g7link.android.ui.theme.G7Colors
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7StatusDot
import app.g7link.android.ui.theme.G7Tints
import app.g7link.android.ui.theme.LocalAppFonts
import android.app.Activity
import android.text.format.DateFormat
import android.view.WindowManager
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
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
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLocale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.g7link.android.R
import app.g7link.android.core.Core
import app.g7link.android.core.Device
import app.g7link.android.ui.remote.RemoteMode
import app.g7link.core.Link
import app.g7link.core.MediaAction
import java.text.SimpleDateFormat
import java.util.Date
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Full-screen ambient Desk Mode dock (`docs/PLAN.md` §3.13 / §8.6 3.4):
 * keeps the display awake with subtle 1-minute OLED burn-in pixel shift,
 * large tabular clock & date, live PC song title/artist/artwork & audio controls,
 * and a balanced 2×2 quick-action dock.
 */
@Composable
fun DeskScreen(
    device: Device,
    core: Core,
    onOpenRemote: (RemoteMode) -> Unit,
    onOpenDeck: () -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    DisposableEffect(Unit) {
        val window = (context as? Activity)?.window
        window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val controller = window?.let { WindowCompat.getInsetsController(it, it.decorView) }
        controller?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller?.hide(WindowInsetsCompat.Type.systemBars())
        onDispose {
            window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            controller?.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    var minuteBucket by remember { mutableLongStateOf(System.currentTimeMillis() / 60_000L) }
    LaunchedEffect(Unit) {
        while (true) {
            val now = System.currentTimeMillis()
            minuteBucket = now / 60_000L
            delay((60_000L - (now % 60_000L)).coerceIn(1_000L, 60_000L))
        }
    }

    // Subtle 1-minute OLED burn-in protection pixel shift (±8 px X, ±6 px Y) for the ambient clock/card.
    val minutePhase = (minuteBucket % 12L).toDouble() * (Math.PI / 6.0)
    val shiftX = (sin(minutePhase) * 8.0).roundToInt()
    val shiftY = (cos(minutePhase) * 6.0).roundToInt()

    val deckState = device.deckState
    val pcMediaSummary by core.pcMedia.summaryFlow.collectAsStateWithLifecycle()
    val activeSummary = pcMediaSummary?.takeIf { it.pcId == device.id } ?: pcMediaSummary
    val artBitmap = remember(activeSummary?.artwork) { activeSummary?.artwork?.asImageBitmap() }

    var draggingVolume by remember { mutableStateOf<Float?>(null) }
    var lastSentVolInt by remember { mutableIntStateOf(-1) }
    val scope = rememberCoroutineScope()

    val tints = G7Tints.dark
    val ink = Color(0xFFF2F2F3)
    val muted = Color(0xFFA1A1A8)
    val raised = Color(0xFF18181B)
    Surface(
        color = G7Colors.deepGround,
        contentColor = ink,
        modifier = modifier.fillMaxSize(),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(start = 24.dp, end = 24.dp, top = 48.dp, bottom = 24.dp),
        ) {
            // The clock, the date and the PC, drifting a little each minute against burn-in.
            Column(Modifier.offset { IntOffset(shiftX, shiftY) }) {
                DeskClockDisplay()
                Row(
                    modifier = Modifier.padding(top = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    G7StatusDot(if (device.online) tints.teal else muted)
                    Text(
                        text = "${device.name} · ${linkText(device.link)}",
                        style = MaterialTheme.typography.labelMedium,
                        color = if (device.online) tints.teal else muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }

            Spacer(Modifier.weight(1f))

            if (device.online && (deckState != null || activeSummary != null)) {
                val isPlaying = activeSummary?.playing ?: (deckState?.playing == true)
                val outName = deckState?.outputDevices?.firstOrNull { it.isDefault }?.name
                    ?: deckState?.outputDevices?.firstOrNull()?.name
                    ?: device.name
                val songTitle = activeSummary?.title?.takeIf { it.isNotBlank() }
                    ?: if (isPlaying) stringResource(R.string.desk_media_playing) else stringResource(R.string.desk_media_audio)
                val subtitleText = buildString {
                    val sub = activeSummary?.subtitle?.trim().orEmpty()
                    if (sub.isNotEmpty()) {
                        append(sub)
                        if (outName.isNotBlank() && !sub.contains(outName, ignoreCase = true)) {
                            append(" · ")
                            append(outName)
                        }
                    } else {
                        append(outName)
                    }
                }
                Spacer(
                    Modifier
                        .fillMaxWidth()
                        .height(1.dp)
                        .background(Color(0xFF222226)),
                )
                Row(
                    modifier = Modifier.fillMaxWidth().padding(vertical = 14.dp),
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (artBitmap != null) {
                        Image(
                            bitmap = artBitmap,
                            contentDescription = null,
                            contentScale = ContentScale.Crop,
                            modifier = Modifier.size(52.dp).clip(RoundedCornerShape(10.dp)),
                        )
                    } else {
                        Box(
                            modifier = Modifier
                                .size(52.dp)
                                .clip(RoundedCornerShape(10.dp))
                                .background(tints.pink.copy(alpha = 0.16f)),
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(G7Icons.music, contentDescription = null, tint = tints.pink, modifier = Modifier.size(22.dp))
                        }
                    }
                    Column(Modifier.weight(1f)) {
                        Text(
                            text = songTitle,
                            style = MaterialTheme.typography.bodyLarge,
                            color = ink,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            text = subtitleText,
                            style = MaterialTheme.typography.bodySmall,
                            color = muted,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    DeskIconButton(
                        icon = G7Icons.previous,
                        label = stringResource(R.string.media_previous),
                        tint = ink,
                        background = raised,
                        onClick = {
                            if (activeSummary != null) {
                                core.pcMedia.pressed(MediaAction.PREVIOUS)
                            } else {
                                scope.launch { core.deckPress(device.id, "media_previous") }
                            }
                        },
                    )
                    DeskIconButton(
                        icon = if (isPlaying) G7Icons.pause else G7Icons.play,
                        label = stringResource(R.string.deck_default_play_pause),
                        tint = ink,
                        background = raised,
                        onClick = {
                            if (activeSummary != null) {
                                core.pcMedia.pressed(if (isPlaying) MediaAction.PAUSE else MediaAction.PLAY)
                            } else {
                                scope.launch { core.deckPress(device.id, "media_play_pause") }
                            }
                        },
                    )
                    DeskIconButton(
                        icon = G7Icons.next,
                        label = stringResource(R.string.media_next),
                        tint = ink,
                        background = raised,
                        onClick = {
                            if (activeSummary != null) {
                                core.pcMedia.pressed(MediaAction.NEXT)
                            } else {
                                scope.launch { core.deckPress(device.id, "media_next") }
                            }
                        },
                    )
                }
                if (deckState != null) {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        DeskIconButton(
                            icon = if (deckState.muted) G7Icons.volumeOff else G7Icons.volume,
                            label = stringResource(if (deckState.muted) R.string.pc_audio_unmute else R.string.pc_audio_mute),
                            tint = tints.teal,
                            background = Color.Transparent,
                            onClick = { core.setPcAudio(device.id, null, !deckState.muted) },
                        )
                        val shownVol = (draggingVolume ?: deckState.volume.toFloat()).coerceIn(0f, 100f)
                        G7Slider(
                            value = shownVol,
                            onValueChange = { v ->
                                draggingVolume = v
                                val rounded = v.roundToInt()
                                if (rounded != lastSentVolInt) {
                                    lastSentVolInt = rounded
                                    core.setPcAudio(device.id, rounded, null)
                                }
                            },
                            onValueChangeFinished = {
                                draggingVolume = null
                                lastSentVolInt = -1
                            },
                            valueRange = 0f..100f,
                            tint = tints.teal,
                            trackColor = Color(0xFF2B2B30),
                            modifier = Modifier.weight(1f),
                        )
                        Text(
                            text = if (deckState.muted) {
                                stringResource(R.string.pc_audio_muted)
                            } else {
                                stringResource(R.string.deck_volume_percent, shownVol.roundToInt())
                            },
                            style = MaterialTheme.typography.labelMedium,
                            color = muted,
                        )
                    }
                }
            } else if (!device.online && device.canWake) {
                DeskIconButton(
                    icon = G7Icons.power,
                    label = stringResource(R.string.action_wake_pc),
                    tint = tints.red,
                    background = raised,
                    onClick = { core.wake(device.id) },
                    text = stringResource(R.string.action_wake_pc),
                    modifier = Modifier.padding(bottom = 12.dp),
                )
            }

            // The dock: the PC's screen, control, Deck, Lock and Exit.
            Row(
                modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                DeskIconButton(
                    icon = G7Icons.screen,
                    label = stringResource(R.string.desk_pc_screen),
                    tint = tints.pink,
                    background = raised,
                    enabled = device.online,
                    onClick = { onOpenRemote(RemoteMode.Screen) },
                    modifier = Modifier.weight(1f),
                )
                DeskIconButton(
                    icon = G7Icons.pointer,
                    label = stringResource(R.string.action_remote_pc),
                    tint = tints.teal,
                    background = raised,
                    enabled = device.online,
                    onClick = { onOpenRemote(RemoteMode.Touchpad) },
                    modifier = Modifier.weight(1f),
                )
                DeskIconButton(
                    icon = G7Icons.deck,
                    label = stringResource(R.string.action_deck_pc),
                    tint = tints.green,
                    background = raised,
                    enabled = device.online,
                    onClick = onOpenDeck,
                    modifier = Modifier.weight(1f),
                )
                DeskIconButton(
                    icon = G7Icons.lock,
                    label = stringResource(R.string.action_lock_pc),
                    tint = tints.teal,
                    background = raised,
                    enabled = device.online,
                    onClick = { core.pcPower(device.id, sleep = false) },
                    modifier = Modifier.weight(1f),
                )
                DeskIconButton(
                    icon = G7Icons.close,
                    label = stringResource(R.string.desk_exit),
                    tint = muted,
                    background = raised,
                    onClick = onBack,
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}

/** A dark button with a tinted icon (and maybe a word), for the desk's dark ground. */
@Composable
private fun DeskIconButton(
    icon: ImageVector,
    label: String,
    tint: Color,
    background: Color,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    text: String? = null,
) {
    Row(
        modifier = modifier
            .clip(RoundedCornerShape(12.dp))
            .background(background)
            .clickable(enabled = enabled, role = Role.Button, onClickLabel = label, onClick = onClick)
            .semantics { contentDescription = label }
            .padding(horizontal = 12.dp, vertical = 14.dp)
            .alpha(if (enabled) 1f else 0.4f),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(22.dp))
        if (text != null) {
            Text(text, style = MaterialTheme.typography.labelLarge, color = Color(0xFFF2F2F3))
        }
    }
}

@Composable
private fun DeskClockDisplay() {
    var nowMs by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            val current = System.currentTimeMillis()
            nowMs = current
            delay((1_000L - (current % 1_000L)).coerceIn(100L, 1_000L))
        }
    }

    val locale = LocalLocale.current.platformLocale
    val timeFormat = remember(locale) { SimpleDateFormat("HH:mm", locale) }
    val secFormat = remember(locale) { SimpleDateFormat(":ss", locale) }
    val dateFormat = remember(locale) {
        val pattern = DateFormat.getBestDateTimePattern(locale, "EEEEMMMMd")
        SimpleDateFormat(pattern, locale)
    }

    val minuteKey = nowMs / 60_000L
    val secondKey = nowMs / 1_000L
    val timeText = remember(minuteKey, locale) { timeFormat.format(Date(minuteKey * 60_000L)) }
    val dateText = remember(minuteKey, locale) { dateFormat.format(Date(minuteKey * 60_000L)) }
    val secText = remember(secondKey, locale) { secFormat.format(Date(secondKey * 1_000L)) }

    val mono = LocalAppFonts.current.mono
    Row(verticalAlignment = Alignment.Bottom) {
        Text(
            text = timeText,
            fontSize = 76.sp,
            lineHeight = 80.sp,
            fontWeight = FontWeight.Normal,
            fontFamily = mono,
            letterSpacing = (-2).sp,
            color = Color(0xFFF2F2F3),
        )
        Text(
            text = secText,
            fontSize = 24.sp,
            fontFamily = mono,
            color = Color(0xFFA1A1A8),
            modifier = Modifier.padding(bottom = 10.dp, start = 4.dp),
        )
    }

    Text(
        text = dateText,
        style = MaterialTheme.typography.bodyLarge,
        color = Color(0xFFA1A1A8),
        modifier = Modifier.padding(top = 8.dp),
    )
}


