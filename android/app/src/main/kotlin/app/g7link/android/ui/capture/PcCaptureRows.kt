// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.capture

import android.os.SystemClock
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.stringResource
import app.g7link.android.R
import app.g7link.android.core.PcCaptureFormat
import app.g7link.android.core.PcCaptureState
import app.g7link.android.ui.theme.G7Icons
import app.g7link.android.ui.theme.G7Row
import app.g7link.android.ui.theme.g7Tints
import kotlinx.coroutines.delay

/** Screenshot and Record screen, for the PC's section on Home. */
@Composable
fun PcCaptureRows(online: Boolean, capture: PcCaptureState, onScreenshot: () -> Unit, onRecord: () -> Unit) {
    val tints = g7Tints()
    G7Row(
        label = stringResource(screenshotLabel(capture)),
        icon = G7Icons.screenshot,
        tint = tints.blue,
        enabled = online,
        busy = capture.shooting,
        onClick = onScreenshot,
    )
    G7Row(
        label = recordLabel(capture),
        icon = G7Icons.video,
        tint = tints.red,
        enabled = online,
        active = capture.recordingSince != null,
        busy = capture.recordingBusy,
        onClick = onRecord,
    )
}

/** "Screenshot", or "Approve on your PC" while the PC asks. */
fun screenshotLabel(capture: PcCaptureState): Int =
    if (capture.shooting && capture.approve) R.string.pc_capture_approve else R.string.pc_capture_screenshot

/** "Record screen", "Stop recording · 0:42", or "Approve on your PC" while the PC asks. */
@Composable
fun recordLabel(capture: PcCaptureState): String {
    val elapsed = rememberElapsed(capture.recordingSince)
    return when {
        capture.recordingBusy && capture.approve -> stringResource(R.string.pc_capture_approve)
        capture.recordingSince != null ->
            stringResource(R.string.pc_capture_stop_recording, PcCaptureFormat.elapsed(elapsed))
        else -> stringResource(R.string.pc_capture_record)
    }
}

/** Milliseconds since [since] ([SystemClock.elapsedRealtime]), ticking each second; 0 without one. */
@Composable
fun rememberElapsed(since: Long?): Long {
    var now by remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(since) {
        while (since != null) {
            now = SystemClock.elapsedRealtime()
            delay(1_000L - (now - since).mod(1_000L))
        }
    }
    return if (since == null) 0L else (now - since).coerceAtLeast(0L)
}
